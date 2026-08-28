//! Content availability provider.
//!
//! This is the capsule-facing `elastos://content/*` contract. The first
//! implementation delegates bytes to the existing low-level IPFS/Kubo backend,
//! then asks the Carrier/provider availability plane to advertise or replicate
//! that CID without exposing raw IPFS/Kubo authority to ordinary capsules.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use elastos_common::protected_content::{
    validate_protected_content_key_envelope_algorithms, SealedObjectV1, SEALED_OBJECT_SCHEMA,
};
use elastos_runtime::provider::{
    Provider, ProviderByteRange, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderProgress, ProviderRegistry, ProviderStreamOptions, ProviderTransfer, ResourceRequest,
    ResourceResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;

const AVAILABILITY_RECEIPT_SCHEMA: &str = "elastos.content.availability.receipt/v1";
const AVAILABILITY_RECEIPT_DOMAIN: &str = "elastos.content.availability.receipt.v1";
const AVAILABILITY_DASHBOARD_SCHEMA: &str = "elastos.content.availability.dashboard/v1";
const CONTENT_ADMISSION_DOMAIN: &str = "elastos.content.admission.v1";
const CONTENT_ACCOUNTING_SCHEMA: &str = "elastos.content.accounting/v1";
const CONTENT_STORAGE_ACCOUNTING_LEDGER_SCHEMA: &str =
    "elastos.content.storage-accounting.ledger/v1";
const CONTENT_STORAGE_ACCOUNTING_ENTRY_SCHEMA: &str = "elastos.content.storage-accounting.entry/v1";
const CONTENT_STORAGE_QUOTA_SCHEMA: &str = "elastos.content.storage-quota/v1";
const CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA: &str =
    "elastos.content.federated-quota-ledger-policy/v1";
const CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA: &str =
    "elastos.content.federated-quota-ledger.exchange-request/v1";
const CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_DOMAIN: &str =
    "elastos.content.federated-quota-ledger.exchange-request.v1";
const CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA: &str =
    "elastos.content.federated-quota-ledger.exchange-receipt/v1";
const CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_DOMAIN: &str =
    "elastos.content.federated-quota-ledger.exchange-receipt.v1";
const CONTENT_ADMISSION_SCHEMA: &str = "elastos.content.admission/v1";
const CONTENT_ABUSE_CONTROLS_SCHEMA: &str = "elastos.content.abuse-controls/v1";
const CONTENT_NETWORK_ABUSE_POLICY_SCHEMA: &str = "elastos.content.network-abuse-policy/v1";
const CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA: &str =
    "elastos.content.federated-abuse-control.exchange-request/v1";
const CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_DOMAIN: &str =
    "elastos.content.federated-abuse-control.exchange-request.v1";
const CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA: &str =
    "elastos.content.federated-abuse-control.exchange-receipt/v1";
const CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_DOMAIN: &str =
    "elastos.content.federated-abuse-control.exchange-receipt.v1";
const CONTENT_OPERATOR_DASHBOARD_SCHEMA: &str = "elastos.content.operator-dashboard/v1";
const CONTENT_FEDERATED_OPERATOR_ALERTING_POLICY_SCHEMA: &str =
    "elastos.content.federated-operator-alerting-policy/v1";
const CONTENT_OPERATOR_ALERT_SCHEMA: &str = "elastos.content.operator-alert/v1";
const CONTENT_OPERATOR_ALERT_RECEIPT_SCHEMA: &str = "elastos.content.operator-alert.receipt/v1";
const CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_REQUEST_SCHEMA: &str =
    "elastos.content.federated-operator-alert.exchange-request/v1";
const CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_RECEIPT_SCHEMA: &str =
    "elastos.content.federated-operator-alert.exchange-receipt/v1";
const CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA: &str =
    "elastos.carrier.peer-attestation-exchange-policy/v1";
const CONTENT_STORAGE_SETTLEMENT_POLICY_SCHEMA: &str =
    "elastos.content.storage-settlement-policy/v1";
const CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA: &str =
    "elastos.content.storage-market-admission-policy/v1";
const CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA: &str =
    "elastos.content.storage-market-admission.request/v1";
const CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA: &str =
    "elastos.content.storage-market-admission.decision/v1";
const REPAIR_TASK_SCHEMA: &str = "elastos.content.repair-task/v1";
const REPAIR_WORKER_RUN_SCHEMA: &str = "elastos.content.repair-worker.run/v1";
const REPAIR_WORKER_ABUSE_CONTROLS_SCHEMA: &str = "elastos.content.repair-worker.abuse-controls/v1";
const REPAIR_FLEET_SCHEMA: &str = "elastos.content.repair-fleet/v1";
const EXTERNAL_REPAIR_FLEET_POLICY_SCHEMA: &str = "elastos.content.external-repair-fleet-policy/v1";
const EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA: &str =
    "elastos.content.external-repair-fleet.dispatch-request/v1";
const EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA: &str =
    "elastos.content.external-repair-fleet.dispatch-receipt/v1";
const REPAIR_RETRY_DELAY_SECS: u64 = 5 * 60;
const REPAIR_HEALTH_CHECK_DELAY_SECS: u64 = 60 * 60;
const REPAIR_WORKER_DEFAULT_LIMIT: usize = 25;
const REPAIR_WORKER_MAX_LIMIT: usize = 100;
const REPAIR_WORKER_DEFAULT_MAX_ATTEMPTS: u32 = 3;
const REPAIR_WORKER_MAX_ATTEMPTS_LIMIT: u32 = 25;
const REPAIR_WORKER_DEFAULT_FAILURE_BUDGET: u32 = 10;
const REPAIR_WORKER_MAX_FAILURE_BUDGET: u32 = 100;
const IMPORT_EXACT_MAX_BYTES: usize = 64 * 1024 * 1024;
const IMPORT_OBJECT_MAX_FILES: usize = 512;
const AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT: usize = 10;
const OBJECT_MANIFEST_SCHEMA: &str = "elastos.content.object.manifest/v1";
const OBJECT_MANIFEST_PATH: &str = "_elastos_object.json";
const SEALED_OBJECT_PATH: &str = "sealed.json";

pub const CONTENT_OBJECT_MANIFEST_PATH: &str = OBJECT_MANIFEST_PATH;

pub struct ContentProvider {
    data_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
    operator_alert_sink: Option<ContentOperatorAlertSink>,
    federated_abuse_control_exchange: Option<ContentFederatedAbuseControlExchangeClient>,
    federated_quota_ledger_exchange: Option<ContentFederatedQuotaLedgerExchangeClient>,
    federated_operator_alert_exchange: Option<ContentFederatedOperatorAlertExchangeClient>,
    storage_market_admission: Option<ContentStorageMarketAdmissionClient>,
    external_repair_fleet: Option<ContentExternalRepairFleetClient>,
}

#[derive(Default)]
pub struct ContentProviderExternalConfigs {
    pub operator_alert_sink: Option<Value>,
    pub storage_market_admission: Option<Value>,
    pub external_repair_fleet: Option<Value>,
    pub federated_operator_alert_exchange: Option<Value>,
    pub federated_quota_ledger_exchange: Option<Value>,
    pub federated_abuse_control_exchange: Option<Value>,
}

#[derive(Debug, Clone)]
struct ContentOperatorAlertSink {
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct ContentFederatedAbuseControlExchangeClient {
    endpoints: Vec<ContentFederatedAbuseControlExchangeEndpoint>,
    quorum: usize,
}

#[derive(Debug, Clone)]
struct ContentFederatedAbuseControlExchangeEndpoint {
    id: String,
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct ContentFederatedQuotaLedgerExchangeClient {
    endpoints: Vec<ContentFederatedQuotaLedgerExchangeEndpoint>,
    quorum: usize,
}

#[derive(Debug, Clone)]
struct ContentFederatedQuotaLedgerExchangeEndpoint {
    id: String,
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct ContentFederatedOperatorAlertExchangeClient {
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct ContentStorageMarketAdmissionClient {
    endpoints: Vec<ContentStorageMarketAdmissionEndpoint>,
    quorum: usize,
}

#[derive(Debug, Clone)]
struct ContentStorageMarketAdmissionEndpoint {
    id: String,
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct ContentExternalRepairFleetClient {
    endpoints: Vec<ContentExternalRepairFleetEndpoint>,
    quorum: usize,
}

#[derive(Debug, Clone)]
struct ContentExternalRepairFleetEndpoint {
    id: String,
    url: String,
    authorization: Option<String>,
    timeout_secs: u64,
}

struct ContentFetchResult {
    payload: ContentFetchPayload,
    availability: Option<Value>,
    transfer: Option<Value>,
}

enum ContentFetchPayload {
    Bytes(String),
    Stream(Value),
}

#[derive(Debug, Clone)]
struct ContentFetchTransfer {
    transfer: ProviderTransfer,
    range: Option<ProviderByteRange>,
    progress: Option<ProviderProgress>,
}

impl ContentFetchTransfer {
    fn from_request(request: &Value) -> Result<Self, ProviderError> {
        let transfer = match request
            .get("transfer")
            .and_then(|value| value.as_str())
            .unwrap_or("bytes")
        {
            "bytes" => ProviderTransfer::Bytes,
            "stream" => ProviderTransfer::Stream,
            "json" => {
                return Err(ProviderError::Provider(
                    "content fetch transfer must be bytes or stream".into(),
                ));
            }
            value => {
                return Err(ProviderError::Provider(format!(
                    "content fetch transfer must be bytes or stream, got {value}"
                )));
            }
        };
        let range = match request.get("range") {
            Some(range) => {
                let start = range
                    .get("start")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| {
                        ProviderError::Provider("content fetch range requires start".into())
                    })?;
                let end = match range.get("end") {
                    Some(value) if value.is_null() => None,
                    Some(value) => Some(value.as_u64().ok_or_else(|| {
                        ProviderError::Provider(
                            "content fetch range end must be an unsigned integer".into(),
                        )
                    })?),
                    None => None,
                };
                Some(ProviderByteRange { start, end })
            }
            None => None,
        };
        let progress = match request.get("progress") {
            Some(progress) => {
                let request_id = progress
                    .get("request_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        ProviderError::Provider("content fetch progress requires request_id".into())
                    })?;
                let expected_bytes = match progress.get("expected_bytes") {
                    Some(value) if value.is_null() => None,
                    Some(value) => Some(value.as_u64().ok_or_else(|| {
                        ProviderError::Provider(
                            "content fetch progress expected_bytes must be an unsigned integer"
                                .into(),
                        )
                    })?),
                    None => None,
                };
                Some(ProviderProgress {
                    request_id: request_id.to_string(),
                    expected_bytes,
                })
            }
            None => None,
        };
        Ok(Self {
            transfer,
            range,
            progress,
        })
    }
}

impl ContentProvider {
    pub fn new(data_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self::new_with_operator_alert_sink_config(data_dir, registry, None)
    }

    pub fn new_with_operator_alert_sink_config(
        data_dir: PathBuf,
        registry: Weak<ProviderRegistry>,
        operator_alert_sink_config: Option<Value>,
    ) -> Self {
        Self::new_with_operator_alert_and_storage_market_config(
            data_dir,
            registry,
            operator_alert_sink_config,
            None,
        )
    }

    pub fn new_with_operator_alert_and_storage_market_config(
        data_dir: PathBuf,
        registry: Weak<ProviderRegistry>,
        operator_alert_sink_config: Option<Value>,
        storage_market_admission_config: Option<Value>,
    ) -> Self {
        Self::new_with_operator_alert_storage_market_and_repair_fleet_config(
            data_dir,
            registry,
            operator_alert_sink_config,
            storage_market_admission_config,
            None,
        )
    }

    pub fn new_with_operator_alert_storage_market_and_repair_fleet_config(
        data_dir: PathBuf,
        registry: Weak<ProviderRegistry>,
        operator_alert_sink_config: Option<Value>,
        storage_market_admission_config: Option<Value>,
        external_repair_fleet_config: Option<Value>,
    ) -> Self {
        Self::new_with_operator_alert_storage_market_repair_fleet_and_alert_exchange_config(
            data_dir,
            registry,
            operator_alert_sink_config,
            storage_market_admission_config,
            external_repair_fleet_config,
            None,
        )
    }

    pub fn new_with_operator_alert_storage_market_repair_fleet_and_alert_exchange_config(
        data_dir: PathBuf,
        registry: Weak<ProviderRegistry>,
        operator_alert_sink_config: Option<Value>,
        storage_market_admission_config: Option<Value>,
        external_repair_fleet_config: Option<Value>,
        federated_operator_alert_exchange_config: Option<Value>,
    ) -> Self {
        Self::new_with_operator_alert_storage_market_repair_fleet_alert_exchange_and_quota_ledger_config(
            data_dir,
            registry,
            operator_alert_sink_config,
            storage_market_admission_config,
            external_repair_fleet_config,
            federated_operator_alert_exchange_config,
            None,
        )
    }

    pub fn new_with_operator_alert_storage_market_repair_fleet_alert_exchange_and_quota_ledger_config(
        data_dir: PathBuf,
        registry: Weak<ProviderRegistry>,
        operator_alert_sink_config: Option<Value>,
        storage_market_admission_config: Option<Value>,
        external_repair_fleet_config: Option<Value>,
        federated_operator_alert_exchange_config: Option<Value>,
        federated_quota_ledger_exchange_config: Option<Value>,
    ) -> Self {
        Self::new_with_external_configs(
            data_dir,
            registry,
            ContentProviderExternalConfigs {
                operator_alert_sink: operator_alert_sink_config,
                storage_market_admission: storage_market_admission_config,
                external_repair_fleet: external_repair_fleet_config,
                federated_operator_alert_exchange: federated_operator_alert_exchange_config,
                federated_quota_ledger_exchange: federated_quota_ledger_exchange_config,
                federated_abuse_control_exchange: None,
            },
        )
    }

    pub fn new_with_external_configs(
        data_dir: PathBuf,
        registry: Weak<ProviderRegistry>,
        configs: ContentProviderExternalConfigs,
    ) -> Self {
        let operator_alert_sink = configs.operator_alert_sink.and_then(|config| {
            match ContentOperatorAlertSink::from_config(config) {
                Ok(sink) => Some(sink),
                Err(err) => {
                    tracing::warn!("content operator alert sink disabled: {}", err);
                    None
                }
            }
        });
        let storage_market_admission = configs.storage_market_admission.and_then(|config| {
            match ContentStorageMarketAdmissionClient::from_config(config) {
                Ok(client) => Some(client),
                Err(err) => {
                    tracing::warn!("content storage-market admission disabled: {}", err);
                    None
                }
            }
        });
        let external_repair_fleet = configs.external_repair_fleet.and_then(|config| {
            match ContentExternalRepairFleetClient::from_config(config) {
                Ok(client) => Some(client),
                Err(err) => {
                    tracing::warn!("content external repair fleet disabled: {}", err);
                    None
                }
            }
        });
        let federated_operator_alert_exchange =
            configs
                .federated_operator_alert_exchange
                .and_then(
                    |config| match ContentFederatedOperatorAlertExchangeClient::from_config(config)
                    {
                        Ok(sink) => Some(sink),
                        Err(err) => {
                            tracing::warn!(
                                "content federated operator alert exchange disabled: {}",
                                err
                            );
                            None
                        }
                    },
                );
        let federated_abuse_control_exchange =
            configs.federated_abuse_control_exchange.and_then(|config| {
                match ContentFederatedAbuseControlExchangeClient::from_config(config) {
                    Ok(client) => Some(client),
                    Err(err) => {
                        tracing::warn!(
                            "content federated abuse-control exchange disabled: {}",
                            err
                        );
                        None
                    }
                }
            });
        let federated_quota_ledger_exchange =
            configs.federated_quota_ledger_exchange.and_then(|config| {
                match ContentFederatedQuotaLedgerExchangeClient::from_config(config) {
                    Ok(client) => Some(client),
                    Err(err) => {
                        tracing::warn!("content federated quota-ledger exchange disabled: {}", err);
                        None
                    }
                }
            });
        Self {
            data_dir,
            registry,
            operator_alert_sink,
            federated_abuse_control_exchange,
            federated_quota_ledger_exchange,
            federated_operator_alert_exchange,
            storage_market_admission,
            external_repair_fleet,
        }
    }

    fn registry(&self) -> Result<Arc<ProviderRegistry>, ProviderError> {
        self.registry.upgrade().ok_or_else(|| {
            ProviderError::Provider("content provider registry unavailable".to_string())
        })
    }

    fn effective_publisher_did(
        &self,
        requested_publisher_did: Option<&str>,
    ) -> Result<String, ProviderError> {
        if let Some(publisher_did) =
            requested_publisher_did.filter(|value| !value.trim().is_empty())
        {
            return Ok(publisher_did.to_string());
        }

        let (_signing_key, default_did) = elastos_identity::load_or_create_did(&self.data_dir)
            .map_err(|err| {
                ProviderError::Provider(format!("content default publisher DID unavailable: {err}"))
            })?;
        Ok(default_did)
    }
}

impl ContentOperatorAlertSink {
    fn from_config(config: Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let url = payload
            .get("url")
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "content operator alert sink requires url".to_string())?;
        validate_operator_alert_sink_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_operator_alert_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        Ok(Self {
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> Value {
        let parsed = url::Url::parse(&self.url).ok();
        json!({
            "configured": true,
            "delivery": "provider_local_webhook",
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn deliver(&self, alert: &Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| format!("operator alert client build failed: {err}"))?;
        let mut request = client.post(&self.url).json(alert);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("operator alert delivery failed: {err}"))?;
        let status = response.status();
        if status.is_success() {
            Ok(json!({
                "configured": true,
                "delivered": true,
                "status": "delivered",
                "http_status": status.as_u16(),
                "sink": self.redacted_status_json(),
            }))
        } else {
            Err(format!(
                "operator alert sink returned HTTP {}",
                status.as_u16()
            ))
        }
    }
}

impl ContentFederatedAbuseControlExchangeClient {
    fn from_config(config: Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let default_authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &default_authorization {
            validate_operator_alert_header_value(value)?;
        }
        let default_timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        let endpoints = match payload.get("endpoints").and_then(|value| value.as_array()) {
            Some(values) if !values.is_empty() => values
                .iter()
                .enumerate()
                .map(|(index, endpoint)| {
                    ContentFederatedAbuseControlExchangeEndpoint::from_config(
                        endpoint,
                        index,
                        default_authorization.as_deref(),
                        default_timeout_secs,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![ContentFederatedAbuseControlExchangeEndpoint::from_config(
                payload,
                0,
                default_authorization.as_deref(),
                default_timeout_secs,
            )?],
        };
        if endpoints.len() > 5 {
            return Err(
                "content federated abuse-control exchange supports at most 5 endpoints".to_string(),
            );
        }
        let quorum = payload
            .get("quorum")
            .or_else(|| payload.get("required_quorum"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(endpoints.len());
        if quorum == 0 || quorum > endpoints.len() {
            return Err(format!(
                "content federated abuse-control exchange quorum must be between 1 and {}",
                endpoints.len()
            ));
        }
        Ok(Self { endpoints, quorum })
    }

    fn redacted_status_json(&self) -> Value {
        let first = self.endpoints.first();
        let parsed = first.and_then(|endpoint| url::Url::parse(&endpoint.url).ok());
        json!({
            "configured": true,
            "delivery": "federated_abuse_control_exchange",
            "endpoint_count": self.endpoints.len(),
            "multi_endpoint": self.endpoints.len() > 1,
            "quorum_required": self.quorum,
            "endpoints": self
                .endpoints
                .iter()
                .map(ContentFederatedAbuseControlExchangeEndpoint::redacted_status_json)
                .collect::<Vec<_>>(),
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.authorization.is_some()),
            "timeout_secs": first.map(|endpoint| endpoint.timeout_secs).unwrap_or(0),
            "credential_exposed": false,
        })
    }

    async fn exchange(&self, signed_request: &Value) -> Result<Value, String> {
        let mut endpoint_receipts = Vec::new();
        let mut accepted_receipts = 0_usize;
        let mut rejected_receipts = 0_usize;
        let mut failed_receipts = 0_usize;
        let mut verified_receipts = 0_usize;
        let mut first_verified_signed_receipt = None;
        let mut reasons = Vec::new();

        for endpoint in &self.endpoints {
            let receipt = endpoint
                .exchange(signed_request)
                .await
                .unwrap_or_else(|err| {
                    failed_receipts = failed_receipts.saturating_add(1);
                    federated_abuse_control_endpoint_unavailable(err, endpoint)
                });
            if receipt
                .get("accepted")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                accepted_receipts = accepted_receipts.saturating_add(1);
            } else if receipt
                .get("signed_receipt")
                .and_then(|value| value.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                rejected_receipts = rejected_receipts.saturating_add(1);
            }
            if receipt
                .get("signed_receipt")
                .and_then(|value| value.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                verified_receipts = verified_receipts.saturating_add(1);
                if first_verified_signed_receipt.is_none() {
                    first_verified_signed_receipt = receipt.get("signed_receipt").cloned();
                }
            }
            if let Some(reason) = receipt.get("reason").and_then(|value| value.as_str()) {
                reasons.push(reason.to_string());
            }
            endpoint_receipts.push(receipt);
        }

        let accepted = accepted_receipts >= self.quorum;
        let reason = if accepted {
            format!(
                "federated abuse-control quorum accepted: {accepted_receipts}/{} verified endpoints accepted",
                self.endpoints.len()
            )
        } else if reasons.is_empty() {
            format!(
                "federated abuse-control quorum rejected: {accepted_receipts}/{} accepted, quorum {}",
                self.endpoints.len(),
                self.quorum
            )
        } else {
            format!(
                "federated abuse-control quorum rejected: {accepted_receipts}/{} accepted, quorum {}; {}",
                self.endpoints.len(),
                self.quorum,
                reasons.join("; ")
            )
        };
        let mut signed_receipt = first_verified_signed_receipt.unwrap_or_else(|| {
            json!({
                "verified": false,
            })
        });
        signed_receipt["verified"] = Value::Bool(verified_receipts > 0);
        signed_receipt["verified_receipts"] = Value::from(verified_receipts);

        Ok(json!({
            "schema": CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA,
            "policy": "configured_federated_abuse_control_exchange",
            "provider": "content-provider",
            "scope": "content-availability",
            "configured": true,
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "exchange": self.redacted_status_json(),
            "quorum": {
                "required": self.quorum,
                "endpoint_count": self.endpoints.len(),
                "accepted": accepted_receipts,
                "rejected": rejected_receipts,
                "failed": failed_receipts,
                "verified": verified_receipts,
            },
            "endpoint_receipts": endpoint_receipts,
            "signed_receipt": signed_receipt,
            "reason": reason,
            "credential_exposed": false,
            "app_visible": false,
        }))
    }
}

impl ContentFederatedAbuseControlExchangeEndpoint {
    fn from_config(
        payload: &Value,
        index: usize,
        default_authorization: Option<&str>,
        default_timeout_secs: u64,
    ) -> Result<Self, String> {
        let url = payload
            .get("url")
            .or_else(|| payload.get("exchange_url"))
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "content federated abuse-control exchange endpoint requires url".to_string()
            })?;
        validate_operator_alert_sink_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(default_authorization)
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_operator_alert_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(default_timeout_secs)
            .clamp(1, 60);
        let id = payload
            .get("id")
            .or_else(|| payload.get("provider_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("abuse-control-{}", index + 1));
        Ok(Self {
            id,
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> Value {
        let parsed = url::Url::parse(&self.url).ok();
        json!({
            "id": self.id,
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn exchange(&self, signed_request: &Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| {
                format!("federated abuse-control exchange client build failed: {err}")
            })?;
        let mut request = client.post(&self.url).json(signed_request);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("federated abuse-control exchange request failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "federated abuse-control exchange returned HTTP {}",
                status.as_u16()
            ));
        }
        let response_json = response.json::<Value>().await.map_err(|err| {
            format!("federated abuse-control exchange response decode failed: {err}")
        })?;
        federated_abuse_control_exchange_receipt_from_response(
            &response_json,
            self.redacted_status_json(),
            status.as_u16(),
        )
    }
}

impl ContentFederatedQuotaLedgerExchangeClient {
    fn from_config(config: Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let default_authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &default_authorization {
            validate_operator_alert_header_value(value)?;
        }
        let default_timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        let endpoints = match payload.get("endpoints").and_then(|value| value.as_array()) {
            Some(values) if !values.is_empty() => values
                .iter()
                .enumerate()
                .map(|(index, endpoint)| {
                    ContentFederatedQuotaLedgerExchangeEndpoint::from_config(
                        endpoint,
                        index,
                        default_authorization.as_deref(),
                        default_timeout_secs,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![ContentFederatedQuotaLedgerExchangeEndpoint::from_config(
                payload,
                0,
                default_authorization.as_deref(),
                default_timeout_secs,
            )?],
        };
        if endpoints.len() > 5 {
            return Err(
                "content federated quota-ledger exchange supports at most 5 endpoints".to_string(),
            );
        }
        let quorum = payload
            .get("quorum")
            .or_else(|| payload.get("required_quorum"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(endpoints.len());
        if quorum == 0 || quorum > endpoints.len() {
            return Err(format!(
                "content federated quota-ledger exchange quorum must be between 1 and {}",
                endpoints.len()
            ));
        }
        Ok(Self { endpoints, quorum })
    }

    fn redacted_status_json(&self) -> Value {
        let first = self.endpoints.first();
        let parsed = first.and_then(|endpoint| url::Url::parse(&endpoint.url).ok());
        json!({
            "configured": true,
            "delivery": "federated_quota_ledger_exchange",
            "endpoint_count": self.endpoints.len(),
            "multi_endpoint": self.endpoints.len() > 1,
            "quorum_required": self.quorum,
            "endpoints": self
                .endpoints
                .iter()
                .map(ContentFederatedQuotaLedgerExchangeEndpoint::redacted_status_json)
                .collect::<Vec<_>>(),
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.authorization.is_some()),
            "timeout_secs": first.map(|endpoint| endpoint.timeout_secs).unwrap_or(0),
            "credential_exposed": false,
        })
    }

    async fn exchange(&self, signed_request: &Value) -> Result<Value, String> {
        let mut endpoint_receipts = Vec::new();
        let mut accepted_receipts = 0_usize;
        let mut rejected_receipts = 0_usize;
        let mut failed_receipts = 0_usize;
        let mut verified_receipts = 0_usize;
        let mut reasons = Vec::new();

        for endpoint in &self.endpoints {
            let receipt = endpoint
                .exchange(signed_request)
                .await
                .unwrap_or_else(|err| {
                    failed_receipts = failed_receipts.saturating_add(1);
                    federated_quota_ledger_endpoint_unavailable(err, endpoint)
                });
            if receipt
                .get("accepted")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                accepted_receipts = accepted_receipts.saturating_add(1);
            } else if receipt
                .get("signed_receipt")
                .and_then(|value| value.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                rejected_receipts = rejected_receipts.saturating_add(1);
            }
            if receipt
                .get("signed_receipt")
                .and_then(|value| value.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                verified_receipts = verified_receipts.saturating_add(1);
            }
            if let Some(reason) = receipt.get("reason").and_then(|value| value.as_str()) {
                reasons.push(reason.to_string());
            }
            endpoint_receipts.push(receipt);
        }

        let accepted = accepted_receipts >= self.quorum;
        let reason = if accepted {
            format!(
                "federated quota-ledger quorum accepted: {accepted_receipts}/{} verified endpoints accepted",
                self.endpoints.len()
            )
        } else if reasons.is_empty() {
            format!(
                "federated quota-ledger quorum rejected: {accepted_receipts}/{} accepted, quorum {}",
                self.endpoints.len(),
                self.quorum
            )
        } else {
            format!(
                "federated quota-ledger quorum rejected: {accepted_receipts}/{} accepted, quorum {}; {}",
                self.endpoints.len(),
                self.quorum,
                reasons.join("; ")
            )
        };

        Ok(json!({
            "schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA,
            "policy": "configured_federated_quota_ledger_exchange",
            "provider": "content-provider",
            "scope": "content-availability",
            "configured": true,
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "exchange": self.redacted_status_json(),
            "quorum": {
                "required": self.quorum,
                "endpoint_count": self.endpoints.len(),
                "accepted": accepted_receipts,
                "rejected": rejected_receipts,
                "failed": failed_receipts,
                "verified": verified_receipts,
            },
            "endpoint_receipts": endpoint_receipts,
            "signed_receipt": {
                "verified": verified_receipts > 0,
                "verified_receipts": verified_receipts,
            },
            "reason": reason,
            "credential_exposed": false,
            "app_visible": false,
        }))
    }
}

impl ContentFederatedQuotaLedgerExchangeEndpoint {
    fn from_config(
        payload: &Value,
        index: usize,
        default_authorization: Option<&str>,
        default_timeout_secs: u64,
    ) -> Result<Self, String> {
        let url = payload
            .get("url")
            .or_else(|| payload.get("exchange_url"))
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "content federated quota-ledger exchange endpoint requires url".to_string()
            })?;
        validate_operator_alert_sink_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(default_authorization)
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_operator_alert_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(default_timeout_secs)
            .clamp(1, 60);
        let id = payload
            .get("id")
            .or_else(|| payload.get("provider_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("quota-ledger-{}", index + 1));
        Ok(Self {
            id,
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> Value {
        let parsed = url::Url::parse(&self.url).ok();
        json!({
            "id": self.id,
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn exchange(&self, signed_request: &Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| format!("federated quota-ledger exchange client build failed: {err}"))?;
        let mut request = client.post(&self.url).json(signed_request);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("federated quota-ledger exchange request failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "federated quota-ledger exchange returned HTTP {}",
                status.as_u16()
            ));
        }
        let response_json = response.json::<Value>().await.map_err(|err| {
            format!("federated quota-ledger exchange response decode failed: {err}")
        })?;
        federated_quota_ledger_exchange_receipt_from_response(
            &response_json,
            self.redacted_status_json(),
            status.as_u16(),
        )
    }
}

impl ContentFederatedOperatorAlertExchangeClient {
    fn from_config(config: Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let url = payload
            .get("url")
            .or_else(|| payload.get("exchange_url"))
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "content federated operator alert exchange requires url".to_string())?;
        validate_operator_alert_sink_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_operator_alert_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        Ok(Self {
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> Value {
        let parsed = url::Url::parse(&self.url).ok();
        json!({
            "configured": true,
            "delivery": "federated_operator_alert_exchange",
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn exchange(&self, request_payload: &Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| {
                format!("federated operator alert exchange client build failed: {err}")
            })?;
        let mut request = client.post(&self.url).json(request_payload);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("federated operator alert exchange request failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "federated operator alert exchange returned HTTP {}",
                status.as_u16()
            ));
        }
        let response_json = response.json::<Value>().await.map_err(|err| {
            format!("federated operator alert exchange response decode failed: {err}")
        })?;
        federated_operator_alert_exchange_receipt_from_response(
            &response_json,
            self,
            status.as_u16(),
        )
    }
}

impl ContentStorageMarketAdmissionClient {
    fn from_config(config: Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let default_authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &default_authorization {
            validate_operator_alert_header_value(value)?;
        }
        let default_timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        let endpoints = match payload.get("endpoints").and_then(|value| value.as_array()) {
            Some(values) if !values.is_empty() => values
                .iter()
                .enumerate()
                .map(|(index, endpoint)| {
                    ContentStorageMarketAdmissionEndpoint::from_config(
                        endpoint,
                        index,
                        default_authorization.as_deref(),
                        default_timeout_secs,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![ContentStorageMarketAdmissionEndpoint::from_config(
                payload,
                0,
                default_authorization.as_deref(),
                default_timeout_secs,
            )?],
        };
        if endpoints.len() > 5 {
            return Err(
                "content storage-market admission supports at most 5 endpoints".to_string(),
            );
        }
        let quorum = payload
            .get("quorum")
            .or_else(|| payload.get("required_quorum"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(endpoints.len());
        if quorum == 0 || quorum > endpoints.len() {
            return Err(format!(
                "content storage-market admission quorum must be between 1 and {}",
                endpoints.len()
            ));
        }
        Ok(Self { endpoints, quorum })
    }

    fn redacted_status_json(&self) -> Value {
        let first = self.endpoints.first();
        let parsed = first.and_then(|endpoint| url::Url::parse(&endpoint.url).ok());
        json!({
            "configured": true,
            "delivery": "external_storage_market_admission",
            "endpoint_count": self.endpoints.len(),
            "multi_endpoint": self.endpoints.len() > 1,
            "quorum_required": self.quorum,
            "endpoints": self
                .endpoints
                .iter()
                .map(ContentStorageMarketAdmissionEndpoint::redacted_status_json)
                .collect::<Vec<_>>(),
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.authorization.is_some()),
            "timeout_secs": first.map(|endpoint| endpoint.timeout_secs).unwrap_or(0),
            "credential_exposed": false,
        })
    }

    async fn decide(&self, request_payload: &Value) -> Result<Value, String> {
        let mut endpoint_decisions = Vec::new();
        let mut accepted_decisions = 0_usize;
        let mut rejected_decisions = 0_usize;
        let mut failed_decisions = 0_usize;
        let mut first_accepted_decision = None;
        let mut reasons = Vec::new();

        for endpoint in &self.endpoints {
            let decision = endpoint
                .decide(request_payload)
                .await
                .unwrap_or_else(|err| {
                    failed_decisions = failed_decisions.saturating_add(1);
                    storage_market_admission_endpoint_unavailable(err, endpoint)
                });
            if decision
                .get("accepted")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                accepted_decisions = accepted_decisions.saturating_add(1);
                if first_accepted_decision.is_none() {
                    first_accepted_decision = Some(decision.clone());
                }
            } else if decision
                .get("status")
                .and_then(|value| value.as_str())
                .is_some_and(|status| status == "rejected")
            {
                rejected_decisions = rejected_decisions.saturating_add(1);
            }
            if let Some(reason) = decision.get("reason").and_then(|value| value.as_str()) {
                reasons.push(reason.to_string());
            }
            endpoint_decisions.push(decision);
        }

        let accepted = accepted_decisions >= self.quorum;
        let reason = if accepted {
            format!(
                "storage-market admission quorum accepted: {accepted_decisions}/{} endpoints accepted",
                self.endpoints.len()
            )
        } else if reasons.is_empty() {
            format!(
                "storage-market admission quorum rejected: {accepted_decisions}/{} accepted, quorum {}",
                self.endpoints.len(),
                self.quorum
            )
        } else {
            format!(
                "storage-market admission quorum rejected: {accepted_decisions}/{} accepted, quorum {}; {}",
                self.endpoints.len(),
                self.quorum,
                reasons.join("; ")
            )
        };
        let first_accepted = first_accepted_decision.unwrap_or(Value::Null);

        Ok(json!({
            "schema": CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA,
            "policy": "external_storage_market_admission",
            "scope": "content-availability",
            "configured": true,
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "reason": reason,
            "market_id": first_accepted.get("market_id").cloned().unwrap_or(Value::Null),
            "offer_id": first_accepted.get("offer_id").cloned().unwrap_or(Value::Null),
            "receipt": first_accepted.get("receipt").cloned().unwrap_or(Value::Null),
            "client": self.redacted_status_json(),
            "quorum": {
                "required": self.quorum,
                "endpoint_count": self.endpoints.len(),
                "accepted": accepted_decisions,
                "rejected": rejected_decisions,
                "failed": failed_decisions,
            },
            "endpoint_decisions": endpoint_decisions,
            "checked_at": now_unix_secs(),
            "app_visible": false,
        }))
    }
}

impl ContentStorageMarketAdmissionEndpoint {
    fn from_config(
        payload: &Value,
        index: usize,
        default_authorization: Option<&str>,
        default_timeout_secs: u64,
    ) -> Result<Self, String> {
        let url = payload
            .get("url")
            .or_else(|| payload.get("admission_url"))
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "content storage-market admission endpoint requires url".to_string())?;
        validate_operator_alert_sink_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(default_authorization)
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_operator_alert_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(default_timeout_secs)
            .clamp(1, 60);
        let id = payload
            .get("id")
            .or_else(|| payload.get("provider_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("storage-market-{}", index + 1));
        Ok(Self {
            id,
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> Value {
        let parsed = url::Url::parse(&self.url).ok();
        json!({
            "id": self.id,
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn decide(&self, request_payload: &Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| format!("storage-market admission client build failed: {err}"))?;
        let mut request = client.post(&self.url).json(request_payload);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("storage-market admission request failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "storage-market admission returned HTTP {}",
                status.as_u16()
            ));
        }
        let response_json = response
            .json::<Value>()
            .await
            .map_err(|err| format!("storage-market admission response decode failed: {err}"))?;
        storage_market_admission_decision_from_response(&response_json, self.redacted_status_json())
    }
}

impl ContentExternalRepairFleetClient {
    fn from_config(config: Value) -> Result<Self, String> {
        let payload = config
            .get("extra")
            .filter(|extra| !extra.is_null())
            .unwrap_or(&config);
        let default_authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(value) = &default_authorization {
            validate_operator_alert_header_value(value)?;
        }
        let default_timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(5)
            .clamp(1, 60);
        let endpoints = match payload.get("endpoints").and_then(|value| value.as_array()) {
            Some(values) if !values.is_empty() => values
                .iter()
                .enumerate()
                .map(|(index, endpoint)| {
                    ContentExternalRepairFleetEndpoint::from_config(
                        endpoint,
                        index,
                        default_authorization.as_deref(),
                        default_timeout_secs,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![ContentExternalRepairFleetEndpoint::from_config(
                payload,
                0,
                default_authorization.as_deref(),
                default_timeout_secs,
            )?],
        };
        if endpoints.len() > 5 {
            return Err("content external repair fleet supports at most 5 endpoints".to_string());
        }
        let quorum = payload
            .get("quorum")
            .or_else(|| payload.get("required_quorum"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(endpoints.len());
        if quorum == 0 || quorum > endpoints.len() {
            return Err(format!(
                "content external repair fleet quorum must be between 1 and {}",
                endpoints.len()
            ));
        }
        Ok(Self { endpoints, quorum })
    }

    fn endpoint_count(&self) -> usize {
        self.endpoints.len()
    }

    fn redacted_status_json(&self) -> Value {
        let first = self.endpoints.first();
        let parsed = first.and_then(|endpoint| url::Url::parse(&endpoint.url).ok());
        json!({
            "configured": true,
            "delivery": "external_repair_fleet_dispatch",
            "endpoint_count": self.endpoints.len(),
            "multi_endpoint": self.endpoints.len() > 1,
            "quorum_required": self.quorum,
            "endpoints": self
                .endpoints
                .iter()
                .map(ContentExternalRepairFleetEndpoint::redacted_status_json)
                .collect::<Vec<_>>(),
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self
                .endpoints
                .iter()
                .any(|endpoint| endpoint.authorization.is_some()),
            "timeout_secs": first.map(|endpoint| endpoint.timeout_secs).unwrap_or(0),
            "credential_exposed": false,
        })
    }

    async fn dispatch(&self, request_payload: &Value) -> Result<Value, String> {
        let mut endpoint_receipts = Vec::new();
        let mut accepted_receipts = 0_usize;
        let mut rejected_receipts = 0_usize;
        let mut failed_receipts = 0_usize;
        let mut first_accepted_receipt = None;
        let mut reasons = Vec::new();

        for endpoint in &self.endpoints {
            let receipt = endpoint
                .dispatch(request_payload)
                .await
                .unwrap_or_else(|err| {
                    failed_receipts = failed_receipts.saturating_add(1);
                    external_repair_fleet_endpoint_dispatch_failed(err, endpoint)
                });
            if receipt
                .get("accepted")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                accepted_receipts = accepted_receipts.saturating_add(1);
                if first_accepted_receipt.is_none() {
                    first_accepted_receipt = Some(receipt.clone());
                }
            } else if receipt
                .get("status")
                .and_then(|value| value.as_str())
                .is_some_and(|status| status == "rejected")
            {
                rejected_receipts = rejected_receipts.saturating_add(1);
            }
            if let Some(reason) = receipt.get("reason").and_then(|value| value.as_str()) {
                reasons.push(reason.to_string());
            }
            endpoint_receipts.push(receipt);
        }

        let accepted = accepted_receipts >= self.quorum;
        let status = if accepted {
            "accepted"
        } else {
            "dispatch_failed"
        };
        let reason = if accepted {
            format!(
                "external repair-fleet quorum accepted: {accepted_receipts}/{} endpoints accepted",
                self.endpoints.len()
            )
        } else if reasons.is_empty() {
            format!(
                "external repair-fleet quorum rejected: {accepted_receipts}/{} accepted, quorum {}",
                self.endpoints.len(),
                self.quorum
            )
        } else {
            format!(
                "external repair-fleet quorum rejected: {accepted_receipts}/{} accepted, quorum {}; {}",
                self.endpoints.len(),
                self.quorum,
                reasons.join("; ")
            )
        };
        let first_accepted = first_accepted_receipt.unwrap_or(Value::Null);

        Ok(json!({
            "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA,
            "policy": "external_repair_fleet_dispatch",
            "scope": "content-availability",
            "configured": true,
            "accepted": accepted,
            "status": status,
            "reason": reason,
            "fleet_id": first_accepted.get("fleet_id").cloned().unwrap_or(Value::Null),
            "job_id": first_accepted.get("job_id").cloned().unwrap_or(Value::Null),
            "receipt": first_accepted.get("receipt").cloned().unwrap_or(Value::Null),
            "client": self.redacted_status_json(),
            "quorum": {
                "required": self.quorum,
                "endpoint_count": self.endpoints.len(),
                "accepted": accepted_receipts,
                "rejected": rejected_receipts,
                "failed": failed_receipts,
            },
            "endpoint_receipts": endpoint_receipts,
            "dispatched_at": now_unix_secs(),
            "app_visible": false,
        }))
    }
}

impl ContentExternalRepairFleetEndpoint {
    fn from_config(
        payload: &Value,
        index: usize,
        default_authorization: Option<&str>,
        default_timeout_secs: u64,
    ) -> Result<Self, String> {
        let url = payload
            .get("url")
            .or_else(|| payload.get("dispatch_url"))
            .or_else(|| payload.get("endpoint_url"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "content external repair fleet endpoint requires url".to_string())?;
        validate_operator_alert_sink_url(url)?;
        let authorization = payload
            .get("authorization")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(default_authorization)
            .map(str::to_string);
        if let Some(value) = &authorization {
            validate_operator_alert_header_value(value)?;
        }
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|value| value.as_u64())
            .unwrap_or(default_timeout_secs)
            .clamp(1, 60);
        let id = payload
            .get("id")
            .or_else(|| payload.get("provider_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("repair-fleet-{}", index + 1));
        Ok(Self {
            id,
            url: url.to_string(),
            authorization,
            timeout_secs,
        })
    }

    fn redacted_status_json(&self) -> Value {
        let parsed = url::Url::parse(&self.url).ok();
        json!({
            "id": self.id,
            "scheme": parsed.as_ref().map(|url| url.scheme()).unwrap_or("unknown"),
            "host": parsed
                .as_ref()
                .and_then(|url| url.host_str())
                .unwrap_or("unknown"),
            "port": parsed.as_ref().and_then(|url| url.port()),
            "path_configured": parsed
                .as_ref()
                .map(|url| !url.path().trim_matches('/').is_empty())
                .unwrap_or(false),
            "authorization_configured": self.authorization.is_some(),
            "timeout_secs": self.timeout_secs,
            "credential_exposed": false,
        })
    }

    async fn dispatch(&self, request_payload: &Value) -> Result<Value, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|err| format!("external repair fleet client build failed: {err}"))?;
        let mut request = client.post(&self.url).json(request_payload);
        if let Some(authorization) = &self.authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("external repair fleet dispatch failed: {err}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "external repair fleet returned HTTP {}",
                status.as_u16()
            ));
        }
        let response_json = response
            .json::<Value>()
            .await
            .map_err(|err| format!("external repair fleet response decode failed: {err}"))?;
        external_repair_fleet_dispatch_receipt_from_response(
            &response_json,
            self.redacted_status_json(),
        )
    }
}

fn validate_operator_alert_sink_url(raw: &str) -> Result<(), String> {
    let url = url::Url::parse(raw).map_err(|err| format!("invalid operator alert URL: {err}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("operator alert URL must not contain inline credentials".to_string());
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => Ok(()),
        _ => Err("operator alert URL must use https or local loopback http".to_string()),
    }
}

fn validate_operator_alert_header_value(value: &str) -> Result<(), String> {
    if value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err("operator alert authorization header contains invalid newline".to_string());
    }
    Ok(())
}

fn federated_operator_alert_exchange_request(alert: &Value, emitted_at: u64) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_REQUEST_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "emitted_at": emitted_at,
        "alert": alert,
        "authority": {
            "runtime_invocation_required": true,
            "provider_owned_exchange": true,
            "credential_exposed": false,
            "raw_backend_access": false,
        },
    })
}

fn federated_operator_alert_exchange_receipt_from_response(
    response: &Value,
    client: &ContentFederatedOperatorAlertExchangeClient,
    http_status: u16,
) -> Result<Value, String> {
    let accepted = response
        .get("accepted")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| {
            "federated operator alert exchange response requires accepted boolean".to_string()
        })?;
    Ok(json!({
        "schema": CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_RECEIPT_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "delivered": accepted,
        "accepted": accepted,
        "status": if accepted { "accepted" } else { "rejected" },
        "http_status": http_status,
        "exchange": client.redacted_status_json(),
        "remote_schema": response.get("schema").cloned().unwrap_or(Value::Null),
        "remote_exchange_id": response.get("exchange_id").cloned().unwrap_or(Value::Null),
        "remote_receipt_id": response.get("receipt_id").cloned().unwrap_or(Value::Null),
        "reason": response.get("reason").cloned().unwrap_or(Value::Null),
        "credential_exposed": false,
    }))
}

fn federated_abuse_control_exchange_request(local_admission: &Value, request: &Value) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "cid": local_admission.get("cid").cloned().unwrap_or(Value::Null),
        "publisher_did": local_admission
            .get("publisher_did")
            .cloned()
            .unwrap_or(Value::Null),
        "estimated_content_bytes": local_admission
            .get("estimated_content_bytes")
            .cloned()
            .unwrap_or(Value::Null),
        "quota": local_admission.get("quota").cloned().unwrap_or(Value::Null),
        "availability_requirements": request
            .get("availability_requirements")
            .cloned()
            .unwrap_or_else(|| json!({})),
        "local_admission": local_admission,
        "requested_at": now_unix_secs(),
        "authority": {
            "runtime_invocation_required": true,
            "provider_owned_exchange": true,
            "preflight_only": true,
            "app_visible": false,
            "credential_exposed": false,
            "raw_backend_access": false,
            "raw_peer_authority": false,
        },
    })
}

fn federated_abuse_control_exchange_receipt_from_response(
    response: &Value,
    exchange: Value,
    http_status: u16,
) -> Result<Value, String> {
    let payload = response
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(response);
    let accepted = payload
        .get("accepted")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| {
            "federated abuse-control exchange response requires accepted boolean".to_string()
        })?;
    let signed_receipt = payload.get("receipt").ok_or_else(|| {
        "federated abuse-control exchange response requires signed receipt".to_string()
    })?;
    let signer_did = signed_receipt
        .get("signer_did")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "federated abuse-control exchange receipt requires signer_did".to_string()
        })?;
    let receipt_bytes = serde_json::to_vec(signed_receipt)
        .map_err(|err| format!("federated abuse-control exchange receipt encode failed: {err}"))?;
    let expected_signers = [signer_did.to_string()];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &receipt_bytes,
        CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_DOMAIN,
        &expected_signers,
    )
    .map_err(|err| {
        format!("federated abuse-control exchange receipt verification failed: {err}")
    })?;
    let receipt_payload = signed_receipt
        .get("payload")
        .filter(|value| value.is_object())
        .ok_or_else(|| "federated abuse-control exchange receipt requires payload".to_string())?;
    if receipt_payload
        .get("schema")
        .and_then(|value| value.as_str())
        != Some(CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA)
    {
        return Err("federated abuse-control exchange receipt schema mismatch".to_string());
    }

    Ok(json!({
        "schema": CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA,
        "policy": "configured_federated_abuse_control_exchange",
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "accepted": accepted,
        "status": if accepted { "accepted" } else { "rejected" },
        "http_status": http_status,
        "exchange": exchange,
        "remote_schema": payload.get("schema").cloned().unwrap_or(Value::Null),
        "remote_exchange_id": payload.get("exchange_id").cloned().unwrap_or(Value::Null),
        "remote_receipt_id": payload.get("receipt_id").cloned().unwrap_or(Value::Null),
        "signed_receipt": {
            "verified": true,
            "signer_did": signer_did,
            "payload_schema": receipt_payload
                .get("schema")
                .cloned()
                .unwrap_or(Value::Null),
            "exchange_id": receipt_payload
                .get("exchange_id")
                .cloned()
                .unwrap_or(Value::Null),
            "receipt_id": receipt_payload
                .get("receipt_id")
                .cloned()
                .unwrap_or(Value::Null),
            "abuse_ledger_id": receipt_payload
                .get("abuse_ledger_id")
                .cloned()
                .unwrap_or(Value::Null),
        },
        "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
        "credential_exposed": false,
        "app_visible": false,
    }))
}

fn federated_abuse_control_exchange_unavailable(
    reason: String,
    client: &ContentFederatedAbuseControlExchangeClient,
) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA,
        "policy": "configured_federated_abuse_control_exchange",
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "abuse_control_unavailable",
        "exchange": client.redacted_status_json(),
        "signed_receipt": {
            "verified": false,
            "reason": reason,
        },
        "reason": reason,
        "credential_exposed": false,
        "app_visible": false,
    })
}

fn federated_abuse_control_endpoint_unavailable(
    reason: String,
    endpoint: &ContentFederatedAbuseControlExchangeEndpoint,
) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA,
        "policy": "configured_federated_abuse_control_exchange",
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "abuse_control_unavailable",
        "exchange": endpoint.redacted_status_json(),
        "signed_receipt": {
            "verified": false,
            "reason": reason,
        },
        "reason": reason,
        "credential_exposed": false,
        "app_visible": false,
    })
}

fn federated_quota_ledger_exchange_request(local_admission: &Value, request: &Value) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "cid": local_admission.get("cid").cloned().unwrap_or(Value::Null),
        "publisher_did": local_admission
            .get("publisher_did")
            .cloned()
            .unwrap_or(Value::Null),
        "estimated_content_bytes": local_admission
            .get("estimated_content_bytes")
            .cloned()
            .unwrap_or(Value::Null),
        "quota": local_admission.get("quota").cloned().unwrap_or(Value::Null),
        "availability_requirements": request
            .get("availability_requirements")
            .cloned()
            .unwrap_or_else(|| json!({})),
        "local_admission": local_admission,
        "requested_at": now_unix_secs(),
        "authority": {
            "runtime_invocation_required": true,
            "provider_owned_exchange": true,
            "preflight_only": true,
            "app_visible": false,
            "credential_exposed": false,
            "raw_backend_access": false,
        },
    })
}

fn federated_quota_ledger_exchange_receipt_from_response(
    response: &Value,
    exchange: Value,
    http_status: u16,
) -> Result<Value, String> {
    let payload = response
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(response);
    let accepted = payload
        .get("accepted")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| {
            "federated quota-ledger exchange response requires accepted boolean".to_string()
        })?;
    let signed_receipt = payload.get("receipt").ok_or_else(|| {
        "federated quota-ledger exchange response requires signed receipt".to_string()
    })?;
    let signer_did = signed_receipt
        .get("signer_did")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "federated quota-ledger exchange receipt requires signer_did".to_string())?;
    let receipt_bytes = serde_json::to_vec(signed_receipt)
        .map_err(|err| format!("federated quota-ledger exchange receipt encode failed: {err}"))?;
    let expected_signers = [signer_did.to_string()];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &receipt_bytes,
        CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_DOMAIN,
        &expected_signers,
    )
    .map_err(|err| format!("federated quota-ledger exchange receipt verification failed: {err}"))?;
    let receipt_payload = signed_receipt
        .get("payload")
        .filter(|value| value.is_object())
        .ok_or_else(|| "federated quota-ledger exchange receipt requires payload".to_string())?;
    if receipt_payload
        .get("schema")
        .and_then(|value| value.as_str())
        != Some(CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA)
    {
        return Err("federated quota-ledger exchange receipt schema mismatch".to_string());
    }

    Ok(json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA,
        "policy": "configured_federated_quota_ledger_exchange",
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "accepted": accepted,
        "status": if accepted { "accepted" } else { "rejected" },
        "http_status": http_status,
        "exchange": exchange,
        "remote_schema": payload.get("schema").cloned().unwrap_or(Value::Null),
        "remote_exchange_id": payload.get("exchange_id").cloned().unwrap_or(Value::Null),
        "remote_receipt_id": payload.get("receipt_id").cloned().unwrap_or(Value::Null),
        "signed_receipt": {
            "verified": true,
            "signer_did": signer_did,
            "payload_schema": receipt_payload
                .get("schema")
                .cloned()
                .unwrap_or(Value::Null),
            "exchange_id": receipt_payload
                .get("exchange_id")
                .cloned()
                .unwrap_or(Value::Null),
            "receipt_id": receipt_payload
                .get("receipt_id")
                .cloned()
                .unwrap_or(Value::Null),
        },
        "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
        "credential_exposed": false,
        "app_visible": false,
    }))
}

fn federated_quota_ledger_exchange_unavailable(
    reason: String,
    client: &ContentFederatedQuotaLedgerExchangeClient,
) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA,
        "policy": "configured_federated_quota_ledger_exchange",
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "quota_ledger_unavailable",
        "exchange": client.redacted_status_json(),
        "signed_receipt": {
            "verified": false,
            "reason": reason,
        },
        "reason": reason,
        "credential_exposed": false,
        "app_visible": false,
    })
}

fn federated_quota_ledger_endpoint_unavailable(
    reason: String,
    endpoint: &ContentFederatedQuotaLedgerExchangeEndpoint,
) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA,
        "policy": "configured_federated_quota_ledger_exchange",
        "provider": "content-provider",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "quota_ledger_unavailable",
        "exchange": endpoint.redacted_status_json(),
        "signed_receipt": {
            "verified": false,
            "reason": reason,
        },
        "reason": reason,
        "credential_exposed": false,
        "app_visible": false,
    })
}

fn storage_market_admission_request(local_admission: &Value, request: &Value) -> Value {
    json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "cid": local_admission.get("cid").cloned().unwrap_or(Value::Null),
        "publisher_did": local_admission
            .get("publisher_did")
            .cloned()
            .unwrap_or(Value::Null),
        "estimated_content_bytes": local_admission
            .get("estimated_content_bytes")
            .cloned()
            .unwrap_or(Value::Null),
        "quota": local_admission.get("quota").cloned().unwrap_or(Value::Null),
        "availability_requirements": request
            .get("availability_requirements")
            .cloned()
            .unwrap_or_else(|| json!({})),
        "local_admission": local_admission,
        "requested_at": now_unix_secs(),
        "app_visible": false,
    })
}

fn storage_market_admission_decision_from_response(
    response: &Value,
    client: Value,
) -> Result<Value, String> {
    let payload = response
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(response);
    let accepted = payload
        .get("accepted")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| "storage-market admission response requires accepted boolean".to_string())?;
    let status = payload
        .get("status")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if accepted {
                "accepted".to_string()
            } else {
                "rejected".to_string()
            }
        });
    Ok(json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA,
        "policy": "external_storage_market_admission",
        "scope": "content-availability",
        "configured": true,
        "accepted": accepted,
        "status": status,
        "reason": payload
            .get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        "market_id": payload
            .get("market_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        "offer_id": payload
            .get("offer_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        "receipt": payload.get("receipt").cloned().unwrap_or(Value::Null),
        "client": client,
        "checked_at": now_unix_secs(),
        "app_visible": false,
    }))
}

fn storage_market_admission_endpoint_unavailable(
    reason: String,
    endpoint: &ContentStorageMarketAdmissionEndpoint,
) -> Value {
    json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA,
        "policy": "external_storage_market_admission",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "market_unavailable",
        "reason": reason,
        "receipt": Value::Null,
        "client": endpoint.redacted_status_json(),
        "checked_at": now_unix_secs(),
        "app_visible": false,
    })
}

fn storage_market_admission_unavailable(reason: String) -> Value {
    json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA,
        "policy": "external_storage_market_admission",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "market_unavailable",
        "reason": reason,
        "receipt": Value::Null,
        "checked_at": now_unix_secs(),
        "app_visible": false,
    })
}

fn external_repair_fleet_dispatch_request(task: &ContentRepairTask, now: u64) -> Value {
    json!({
        "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "cid": task.cid,
        "uri": format!("elastos://{}", task.cid),
        "object_did": task.object_did.clone(),
        "publisher_did": task.publisher_did.clone(),
        "availability_policy": task.policy.clone(),
        "availability_requirements": task.requirements.clone(),
        "repair_task": {
            "schema": REPAIR_TASK_SCHEMA,
            "status": task.status.clone(),
            "attempts": task.attempts,
            "next_check_after": task.next_check_after,
            "checked_at": task.checked_at,
        },
        "requested_at": now,
        "runtime_invocation_required": true,
        "app_visible": false,
    })
}

fn external_repair_fleet_dispatch_receipt_from_response(
    response: &Value,
    client: Value,
) -> Result<Value, String> {
    let payload = response
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(response);
    let accepted = payload
        .get("accepted")
        .and_then(|value| value.as_bool())
        .ok_or_else(|| "external repair fleet response requires accepted boolean".to_string())?;
    let status = payload
        .get("status")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if accepted {
                "accepted".to_string()
            } else {
                "rejected".to_string()
            }
        });
    Ok(json!({
        "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA,
        "policy": "external_repair_fleet_dispatch",
        "scope": "content-availability",
        "configured": true,
        "accepted": accepted,
        "status": status,
        "reason": payload
            .get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        "fleet_id": payload
            .get("fleet_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        "job_id": payload
            .get("job_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        "receipt": payload.get("receipt").cloned().unwrap_or(Value::Null),
        "client": client,
        "dispatched_at": now_unix_secs(),
        "app_visible": false,
    }))
}

fn external_repair_fleet_endpoint_dispatch_failed(
    reason: String,
    endpoint: &ContentExternalRepairFleetEndpoint,
) -> Value {
    json!({
        "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA,
        "policy": "external_repair_fleet_dispatch",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "dispatch_failed",
        "reason": reason,
        "receipt": Value::Null,
        "client": endpoint.redacted_status_json(),
        "dispatched_at": now_unix_secs(),
        "app_visible": false,
    })
}

fn external_repair_fleet_dispatch_failed(reason: String) -> Value {
    json!({
        "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA,
        "policy": "external_repair_fleet_dispatch",
        "scope": "content-availability",
        "configured": true,
        "accepted": false,
        "status": "dispatch_failed",
        "reason": reason,
        "receipt": Value::Null,
        "dispatched_at": now_unix_secs(),
        "app_visible": false,
    })
}

pub async fn publish_directory_via_provider(
    registry: &ProviderRegistry,
    dir: &Path,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
) -> anyhow::Result<String> {
    publish_directory_via_provider_with_kind(registry, dir, "directory", object_did, publisher_did)
        .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentPublishRequirements {
    min_replicas: u32,
    require_live_multi_peer_proof: bool,
}

impl ContentPublishRequirements {
    pub(crate) fn new(
        min_replicas: u32,
        require_live_multi_peer_proof: bool,
    ) -> anyhow::Result<Self> {
        if min_replicas == 0 {
            anyhow::bail!("content publish minimum replicas must be positive");
        }
        Ok(Self {
            min_replicas,
            require_live_multi_peer_proof,
        })
    }

    fn to_json(self) -> Value {
        json!({
            "min_replicas": self.min_replicas,
            "require_live_multi_peer_proof": self.require_live_multi_peer_proof,
        })
    }
}

pub async fn publish_directory_via_provider_with_kind(
    registry: &ProviderRegistry,
    dir: &Path,
    object_kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
) -> anyhow::Result<String> {
    publish_directory_via_provider_impl(
        registry,
        dir,
        object_kind,
        object_did,
        publisher_did,
        &[],
        None,
    )
    .await
}

pub(crate) async fn publish_directory_via_provider_with_kind_and_requirements(
    registry: &ProviderRegistry,
    dir: &Path,
    object_kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    requirements: ContentPublishRequirements,
) -> anyhow::Result<String> {
    publish_directory_via_provider_impl(
        registry,
        dir,
        object_kind,
        object_did,
        publisher_did,
        &[],
        Some(requirements),
    )
    .await
}

pub async fn publish_directory_via_provider_with_kind_and_links(
    registry: &ProviderRegistry,
    dir: &Path,
    object_kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    links: &[(String, String)],
) -> anyhow::Result<String> {
    publish_directory_via_provider_impl(
        registry,
        dir,
        object_kind,
        object_did,
        publisher_did,
        links,
        None,
    )
    .await
}

async fn publish_directory_via_provider_impl(
    registry: &ProviderRegistry,
    dir: &Path,
    object_kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    links: &[(String, String)],
    requirements: Option<ContentPublishRequirements>,
) -> anyhow::Result<String> {
    let mut files = Vec::new();
    crate::ipfs::collect_files_for_ipfs(dir, dir, &mut files)?;
    if files.is_empty() {
        anyhow::bail!("No files found in {}", dir.display());
    }

    let mut entries = Vec::new();
    for rel_path in &files {
        let abs_path = dir.join(rel_path);
        let bytes = std::fs::read(&abs_path)?;
        entries.push(json!({
            "path": rel_path.to_string_lossy().replace('\\', "/"),
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }));
    }

    let mut request = json!({
        "op": "publish",
        "kind": "directory",
        "object_kind": object_kind,
        "files": entries,
        "pin": true,
    });
    if let Some(object_did) = object_did {
        request["object_did"] = Value::String(object_did.to_string());
    }
    if let Some(publisher_did) = publisher_did {
        request["publisher_did"] = Value::String(publisher_did.to_string());
    }
    if let Some(requirements) = requirements {
        request["availability_requirements"] = requirements.to_json();
    }
    if !links.is_empty() {
        request["links"] = Value::Array(
            links
                .iter()
                .map(|(rel, cid)| {
                    json!({
                        "rel": rel,
                        "cid": cid,
                    })
                })
                .collect(),
        );
    }

    let response = registry
        .send_raw("content", &request)
        .await
        .map_err(|err| anyhow::anyhow!("content provider unavailable: {err}"))?;
    content_response_cid(&response)
}

pub async fn publish_bytes_via_provider(
    registry: &ProviderRegistry,
    filename: &str,
    bytes: &[u8],
    object_did: Option<&str>,
    publisher_did: Option<&str>,
) -> anyhow::Result<String> {
    let mut request = json!({
        "op": "publish",
        "kind": "file",
        "filename": filename,
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        "pin": true,
    });
    if let Some(object_did) = object_did {
        request["object_did"] = Value::String(object_did.to_string());
    }
    if let Some(publisher_did) = publisher_did {
        request["publisher_did"] = Value::String(publisher_did.to_string());
    }

    let response = registry
        .send_raw("content", &request)
        .await
        .map_err(|err| anyhow::anyhow!("content provider unavailable: {err}"))?;
    content_response_cid(&response)
}

pub async fn fetch_bytes_via_provider(
    registry: &ProviderRegistry,
    cid: &str,
    path: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let mut request = json!({
        "op": "fetch",
        "cid": cid,
        "transfer": "stream",
        "progress": {
            "request_id": format!("content-fetch:{cid}:{}", path.unwrap_or("root")),
            "expected_bytes": Value::Null,
        },
    });
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        request["path"] = Value::String(path.to_string());
    }

    let mut session = registry
        .open_provider_stream(
            ProviderInvocation {
                source: "runtime-content-consumer".to_string(),
                target: "content".to_string(),
                op: "fetch".to_string(),
                request,
                transfer: ProviderTransfer::Stream,
                range: None,
                progress: Some(ProviderProgress {
                    request_id: format!("content-fetch:{cid}:{}", path.unwrap_or("root")),
                    expected_bytes: None,
                }),
                transport: ProviderInvocationTransport::Local,
            },
            ProviderStreamOptions::default(),
        )
        .await
        .map_err(|err| anyhow::anyhow!("content provider stream unavailable: {err}"))?;
    session
        .drain_to_vec()
        .map_err(|err| anyhow::anyhow!("content provider stream read failed: {err}"))
}

pub async fn fetch_content_object_manifest(
    registry: &ProviderRegistry,
    cid: &str,
) -> anyhow::Result<ContentObjectManifest> {
    let bytes = fetch_bytes_via_provider(registry, cid, Some(CONTENT_OBJECT_MANIFEST_PATH)).await?;
    parse_content_object_manifest(cid, &bytes)
}

pub fn parse_content_object_manifest(
    cid: &str,
    bytes: &[u8],
) -> anyhow::Result<ContentObjectManifest> {
    let manifest: ContentObjectManifest = serde_json::from_slice(bytes).map_err(|err| {
        anyhow::anyhow!("content object {cid} has invalid {CONTENT_OBJECT_MANIFEST_PATH}: {err}")
    })?;
    if manifest.schema != OBJECT_MANIFEST_SCHEMA {
        anyhow::bail!(
            "content object {cid} uses unsupported object manifest schema {}",
            manifest.schema
        );
    }
    Ok(manifest)
}

/// Materialize a published capsule through the content availability contract.
///
/// Data capsules must carry `_elastos_object.json`; that manifest is the file
/// list and integrity contract above the low-level block backend.
pub async fn prepare_capsule_from_content_provider(
    registry: &ProviderRegistry,
    cid: &str,
) -> anyhow::Result<PathBuf> {
    let manifest_bytes = match fetch_bytes_via_provider(registry, cid, Some("capsule.json")).await {
        Ok(bytes) => bytes,
        Err(capsule_err) => {
            if let Ok(object_manifest) = fetch_content_object_manifest(registry, cid).await {
                anyhow::bail!(
                    "content object {cid} has kind '{}' and is not a launchable capsule; use `elastos open elastos://{cid}` to inspect release objects or open it with a matching viewer once one is installed",
                    object_manifest.kind
                );
            }
            return Err(capsule_err);
        }
    };
    let manifest_data = String::from_utf8(manifest_bytes.clone())
        .map_err(|err| anyhow::anyhow!("Manifest is not valid UTF-8 for CID {}: {}", cid, err))?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)?;
    manifest
        .validate()
        .map_err(|err| anyhow::anyhow!("Invalid manifest from CID {}: {}", cid, err))?;

    tracing::info!(
        "Loading capsule '{}' ({:?}) through content availability",
        manifest.name,
        manifest.capsule_type
    );

    let temp_dir = tempfile::Builder::new()
        .prefix("elastos-capsule-")
        .tempdir()?;
    let capsule_dir = temp_dir.path().to_path_buf();
    write_materialized_file(&capsule_dir, "capsule.json", &manifest_bytes).await?;

    match manifest.capsule_type {
        elastos_common::CapsuleType::MicroVM => {
            anyhow::bail!(
                "MicroVM capsule opens still require the explicit operator path until content availability supports streamed large-object materialization"
            );
        }
        elastos_common::CapsuleType::Data => {
            materialize_data_capsule(registry, cid, &manifest, &manifest_bytes, &capsule_dir)
                .await?;
        }
        _ => {
            let entrypoint_bytes =
                fetch_bytes_via_provider(registry, cid, Some(&manifest.entrypoint)).await?;
            write_materialized_file(&capsule_dir, &manifest.entrypoint, &entrypoint_bytes).await?;
        }
    }

    Ok(temp_dir.keep())
}

#[async_trait]
impl Provider for ContentProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "content provider only supports capability-scoped raw operations".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["content"]
    }

    fn name(&self) -> &'static str {
        "content-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        match request.get("op").and_then(|op| op.as_str()) {
            Some("publish") => self.publish(request).await,
            Some("fetch") => self.fetch(request).await,
            Some("import_exact") => self.import_exact(request).await,
            Some("import_object") => self.import_object(request).await,
            Some("admission") => self.admission(request).await,
            Some("ensure") => self.ensure(request).await,
            Some("repair") => self.repair(request).await,
            Some("repair_worker") => self.run_repair_worker(request).await,
            Some("unpublish") => self.unpublish(request).await,
            Some("status") => self.status(request).await,
            Some(op) => Ok(provider_error(
                "unsupported_operation",
                &format!("unsupported content operation: {op}"),
            )),
            None => Ok(provider_error(
                "invalid_request",
                "missing content operation",
            )),
        }
    }
}

impl ContentProvider {
    async fn invoke_provider(
        &self,
        registry: &ProviderRegistry,
        target: &str,
        op: &str,
        request: Value,
        transfer: ProviderTransfer,
    ) -> Result<Value, ProviderError> {
        registry
            .invoke_provider(ProviderInvocation {
                source: self.name().to_string(),
                target: target.to_string(),
                op: op.to_string(),
                request,
                transfer,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
    }

    async fn invoke_provider_with_fetch_transfer(
        &self,
        registry: &ProviderRegistry,
        target: &str,
        op: &str,
        request: Value,
        transfer: &ContentFetchTransfer,
    ) -> Result<Value, ProviderError> {
        registry
            .invoke_provider(ProviderInvocation {
                source: self.name().to_string(),
                target: target.to_string(),
                op: op.to_string(),
                request,
                transfer: transfer.transfer,
                range: transfer.range,
                progress: transfer.progress.clone(),
                transport: ProviderInvocationTransport::Local,
            })
            .await
    }

    async fn fetch(&self, request: &Value) -> Result<Value, ProviderError> {
        let cid = request
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content fetch requires cid".into()))?;
        if !is_valid_cid(cid) {
            return Ok(provider_error(
                "invalid_cid",
                "content fetch requires a valid CID",
            ));
        }

        let path = request
            .get("path")
            .and_then(|path| path.as_str())
            .unwrap_or("");
        if let Err(message) = validate_content_path(path) {
            return Ok(provider_error("invalid_path", &message));
        }

        let registry = self.registry()?;
        let transfer = ContentFetchTransfer::from_request(request)?;
        let result = match self
            .fetch_from_local_backend(&registry, cid, path, &transfer)
            .await
        {
            Ok(result) => result,
            Err(local_err)
                if request.get("local_only").and_then(|value| value.as_bool()) == Some(true) =>
            {
                return Err(local_err);
            }
            Err(local_err) => match self
                .fetch_from_availability_provider(&registry, cid, path, &transfer)
                .await
            {
                Ok(Some(result)) => result,
                Ok(None) => return Err(local_err),
                Err(availability_err) => {
                    return Err(ProviderError::Provider(format!(
                        "{local_err}; availability fetch failed: {availability_err}"
                    )))
                }
            },
        };

        let receipt_availability = self
            .latest_receipt_for_cid(cid)
            .transpose()?
            .map(|receipt| {
                json!({
                    "status": receipt.payload.status,
                    "provider": receipt.payload.provider,
                    "replicas": receipt.payload.replicas,
                    "checked_at": receipt.payload.checked_at,
                })
            })
            .unwrap_or_else(|| {
                json!({
                    "status": "unknown",
                    "provider": "content-provider",
                })
            });
        let availability = result.availability.unwrap_or(receipt_availability);

        let mut response = json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "path": path,
            "availability": availability,
        });
        match result.payload {
            ContentFetchPayload::Bytes(data) => {
                response["data"] = Value::String(data);
            }
            ContentFetchPayload::Stream(stream) => {
                response["stream"] = stream;
            }
        }
        if let Some(transfer) = result.transfer {
            response["transfer"] = transfer;
        }
        Ok(provider_ok(response))
    }

    async fn fetch_from_local_backend(
        &self,
        registry: &ProviderRegistry,
        cid: &str,
        path: &str,
        transfer: &ContentFetchTransfer,
    ) -> Result<ContentFetchResult, ProviderError> {
        let mut ipfs_request = json!({
            "op": "cat",
            "cid": cid,
        });
        if !path.is_empty() {
            ipfs_request["path"] = Value::String(path.to_string());
        }

        let ipfs_response = self
            .invoke_provider_with_fetch_transfer(registry, "ipfs", "cat", ipfs_request, transfer)
            .await?;
        provider_response_ok(&ipfs_response, "content fetch")?;
        Ok(ContentFetchResult {
            payload: provider_response_payload(&ipfs_response, "content backend", transfer)?,
            availability: None,
            transfer: provider_transfer_value(&ipfs_response),
        })
    }

    async fn fetch_from_availability_provider(
        &self,
        registry: &ProviderRegistry,
        cid: &str,
        path: &str,
        transfer: &ContentFetchTransfer,
    ) -> Result<Option<ContentFetchResult>, ProviderError> {
        let mut request = json!({
            "op": "fetch",
            "cid": cid,
            "uri": format!("elastos://{cid}"),
        });
        if !path.is_empty() {
            request["path"] = Value::String(path.to_string());
        }

        let response = match self
            .invoke_provider_with_fetch_transfer(
                registry,
                "availability",
                "fetch",
                request,
                transfer,
            )
            .await
        {
            Ok(response) => response,
            Err(ProviderError::NoProvider(_)) => return Ok(None),
            Err(err) => return Err(err),
        };
        if response.get("status").and_then(|status| status.as_str()) == Some("error") {
            return Ok(None);
        }
        let availability = response
            .get("data")
            .and_then(|data| data.get("availability"))
            .cloned();
        Ok(Some(ContentFetchResult {
            payload: provider_response_payload(&response, "availability provider", transfer)?,
            availability,
            transfer: provider_transfer_value(&response),
        }))
    }

    async fn publish(&self, request: &Value) -> Result<Value, ProviderError> {
        let kind = request.get("kind").and_then(|kind| kind.as_str());
        let pin = request
            .get("pin")
            .and_then(|pin| pin.as_bool())
            .unwrap_or(true);
        let registry = self.registry()?;

        let ipfs_request = match kind {
            Some("directory") => {
                let files = request
                    .get("files")
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()));
                if !files.is_array() {
                    return Ok(provider_error("invalid_request", "files must be an array"));
                }
                let files = with_directory_object_manifest(
                    files,
                    request
                        .get("object_kind")
                        .and_then(|value| value.as_str())
                        .unwrap_or("directory"),
                    request.get("object_did").and_then(|value| value.as_str()),
                    request
                        .get("publisher_did")
                        .and_then(|value| value.as_str()),
                    request.get("links"),
                )?;
                json!({
                    "op": "add_directory",
                    "files": files,
                    "pin": pin,
                })
            }
            Some("file") => {
                let data = request
                    .get("data")
                    .and_then(|data| data.as_str())
                    .filter(|data| !data.trim().is_empty())
                    .ok_or_else(|| {
                        ProviderError::Provider("content file publish requires data".into())
                    })?;
                let filename = request
                    .get("filename")
                    .and_then(|filename| filename.as_str())
                    .filter(|filename| !filename.trim().is_empty())
                    .unwrap_or("content.bin");
                json!({
                    "op": "add_bytes",
                    "data": data,
                    "filename": filename,
                    "pin": pin,
                })
            }
            Some(_) | None => {
                return Ok(provider_error(
                    "unsupported_content_kind",
                    "content publish supports kind=directory or kind=file",
                ));
            }
        };

        let ipfs_op = ipfs_request
            .get("op")
            .and_then(|value| value.as_str())
            .unwrap_or("publish")
            .to_string();
        let accounting_observation = content_accounting_observation_from_publish_request(request);
        let requirements = AvailabilityRequirements::from_request(request);
        let object_did = request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = Some(self.effective_publisher_did(publisher_did.as_deref())?);
        let storage_quota = self.principal_storage_quota_for_request(
            publisher_did.as_deref().unwrap_or_default(),
            &requirements,
            if pin {
                accounting_observation.bytes
            } else {
                Some(0)
            },
            None,
        )?;
        if storage_quota.get("status").and_then(|value| value.as_str()) == Some("quota_exceeded") {
            return Ok(provider_error(
                "storage_quota_exceeded",
                "content publish exceeds the principal storage quota",
            ));
        }
        let ipfs_response = self
            .invoke_provider(
                &registry,
                "ipfs",
                &ipfs_op,
                ipfs_request,
                ProviderTransfer::Bytes,
            )
            .await?;
        let cid = provider_response_cid(&ipfs_response)?;
        let local_outcome = AvailabilityOutcome::local_publish(pin);
        let outcome = if pin {
            self.ensure_network_availability(
                &registry,
                &cid,
                request,
                &local_outcome,
                AvailabilityRequestContext {
                    object_did: object_did.as_deref(),
                    publisher_did: publisher_did.as_deref(),
                    accounting_observation,
                },
            )
            .await?
            .unwrap_or(local_outcome)
        } else {
            local_outcome
        };
        let receipt = self.write_receipt(ReceiptInput {
            cid: cid.clone(),
            object_did,
            publisher_did,
            provider: outcome.provider.clone(),
            policy: outcome.policy.clone(),
            status: outcome.status.clone(),
            replicas: outcome.replicas,
            peer_selection: outcome.peer_selection.clone(),
            quota: outcome.quota.clone(),
            repair_worker: outcome.repair_worker.clone(),
            storage_market: outcome.storage_market.clone(),
            repair_graph: outcome.repair_graph.clone(),
            abuse_controls: outcome.abuse_controls.clone(),
            accounting: content_accounting_json_with_storage_quota(
                "publish_request",
                accounting_observation,
                outcome.replicas,
                storage_quota,
            ),
        })?;
        let repair_task = self.record_repair_task(&receipt, &outcome, requirements, false)?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "repair_task": repair_task,
            "receipt": receipt,
        })))
    }

    async fn admission(&self, request: &Value) -> Result<Value, ProviderError> {
        let cid = request
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content admission requires cid".into()))?;
        if !is_valid_cid(cid) {
            return Ok(provider_error(
                "invalid_cid",
                "content admission requires a valid CID",
            ));
        }

        let requirements = AvailabilityRequirements::from_request(request);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = self.effective_publisher_did(publisher_did.as_deref())?;
        let incoming_content_bytes = admission_content_bytes_from_request(request);
        let mut storage_quota = if requirements.max_storage_bytes_per_principal.is_some()
            && incoming_content_bytes.is_none()
        {
            json!({
                "schema": CONTENT_STORAGE_QUOTA_SCHEMA,
                "policy": "principal_storage_quota",
                "scope": "content-availability",
                "enforced": true,
                "status": "known_size_required",
                "principal_did": publisher_did,
                "reason": "remote admission with a storage quota requires estimated content bytes before transfer",
            })
        } else {
            self.principal_storage_quota_for_request(
                &publisher_did,
                &requirements,
                incoming_content_bytes,
                Some(cid),
            )?
        };
        let quota_status = storage_quota
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let accepted = !matches!(quota_status, "quota_exceeded" | "known_size_required");
        let reason = match quota_status {
            "quota_exceeded" => {
                Some("remote content replica would exceed the principal storage quota")
            }
            "known_size_required" => Some("remote content admission requires known content bytes"),
            _ => None,
        };

        let mut admission = json!({
            "schema": CONTENT_ADMISSION_SCHEMA,
            "policy": "content_provider_principal_quota_preflight",
            "scope": "content-availability",
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "reason": reason,
            "cid": cid,
            "publisher_did": publisher_did,
            "estimated_content_bytes": incoming_content_bytes,
            "quota": storage_quota,
            "checked_at": now_unix_secs(),
            "app_visible": false,
        });
        if accepted {
            if let Some(federated_abuse_control_exchange) = &self.federated_abuse_control_exchange {
                let signed_exchange_request =
                    self.sign_federated_abuse_control_exchange_request(&admission, request)?;
                let abuse_control_exchange = federated_abuse_control_exchange
                    .exchange(&signed_exchange_request)
                    .await
                    .unwrap_or_else(|err| {
                        federated_abuse_control_exchange_unavailable(
                            err,
                            federated_abuse_control_exchange,
                        )
                    });
                if abuse_control_exchange
                    .get("accepted")
                    .and_then(|value| value.as_bool())
                    != Some(true)
                {
                    admission["accepted"] = Value::Bool(false);
                    admission["status"] = Value::String("rejected".to_string());
                    let abuse_reason = abuse_control_exchange
                        .get("reason")
                        .and_then(|value| value.as_str())
                        .unwrap_or("federated abuse control rejected admission");
                    admission["reason"] = Value::String(format!(
                        "federated abuse control rejected admission: {abuse_reason}"
                    ));
                }
                admission["federated_abuse_control_exchange"] = abuse_control_exchange;
            }
        }
        if admission.get("accepted").and_then(|value| value.as_bool()) == Some(true) {
            if let Some(federated_quota_ledger_exchange) = &self.federated_quota_ledger_exchange {
                let signed_exchange_request =
                    self.sign_federated_quota_ledger_exchange_request(&admission, request)?;
                let quota_ledger_exchange = federated_quota_ledger_exchange
                    .exchange(&signed_exchange_request)
                    .await
                    .unwrap_or_else(|err| {
                        federated_quota_ledger_exchange_unavailable(
                            err,
                            federated_quota_ledger_exchange,
                        )
                    });
                let quota_ledger_policy = federated_quota_ledger_policy_from_exchange(
                    &storage_quota,
                    &quota_ledger_exchange,
                );
                if let Some(quota) = storage_quota.as_object_mut() {
                    quota.insert(
                        "federated_quota_ledger_policy".to_string(),
                        quota_ledger_policy,
                    );
                }
                admission["quota"] = storage_quota.clone();
                if quota_ledger_exchange
                    .get("accepted")
                    .and_then(|value| value.as_bool())
                    != Some(true)
                {
                    admission["accepted"] = Value::Bool(false);
                    admission["status"] = Value::String("rejected".to_string());
                    let quota_reason = quota_ledger_exchange
                        .get("reason")
                        .and_then(|value| value.as_str())
                        .unwrap_or("federated quota ledger rejected admission");
                    admission["reason"] = Value::String(format!(
                        "federated quota ledger rejected admission: {quota_reason}"
                    ));
                }
                admission["federated_quota_ledger_exchange"] = quota_ledger_exchange;
            }
        }
        if admission.get("accepted").and_then(|value| value.as_bool()) == Some(true) {
            if let Some(storage_market_admission) = &self.storage_market_admission {
                let market_request = storage_market_admission_request(&admission, request);
                let market_decision = storage_market_admission
                    .decide(&market_request)
                    .await
                    .unwrap_or_else(storage_market_admission_unavailable);
                if market_decision
                    .get("accepted")
                    .and_then(|value| value.as_bool())
                    != Some(true)
                {
                    admission["accepted"] = Value::Bool(false);
                    admission["status"] = Value::String("rejected".to_string());
                    let market_reason = market_decision
                        .get("reason")
                        .and_then(|value| value.as_str())
                        .unwrap_or("external storage market rejected admission");
                    admission["reason"] = Value::String(format!(
                        "storage market admission rejected: {market_reason}"
                    ));
                }
                admission["storage_market_admission"] = market_decision;
            }
        }
        let receipt = self.sign_admission_receipt(&admission)?;

        Ok(provider_ok(json!({
            "cid": cid,
            "admission": admission,
            "receipt": receipt,
        })))
    }

    async fn import_exact(&self, request: &Value) -> Result<Value, ProviderError> {
        validate_import_exact_invocation(request)?;
        let cid = request
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content import_exact requires cid".into()))?;
        if !is_valid_cid(cid) {
            return Ok(provider_error(
                "invalid_cid",
                "content import_exact requires a valid CID",
            ));
        }
        let bytes = import_exact_payload_bytes(request)?;
        let requirements = AvailabilityRequirements::from_request(request);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = Some(self.effective_publisher_did(publisher_did.as_deref())?);
        let storage_quota = self.principal_storage_quota_for_request(
            publisher_did.as_deref().unwrap_or_default(),
            &requirements,
            Some(bytes.len() as u64),
            Some(cid),
        )?;
        if storage_quota.get("status").and_then(|value| value.as_str()) == Some("quota_exceeded") {
            return Ok(provider_error(
                "storage_quota_exceeded",
                "content import_exact exceeds the principal storage quota",
            ));
        }
        let filename = request
            .get("filename")
            .and_then(|filename| filename.as_str())
            .filter(|filename| !filename.trim().is_empty())
            .unwrap_or("content.bin");
        let registry = self.registry()?;
        let ipfs_response = self
            .invoke_provider(
                &registry,
                "ipfs",
                "add_bytes",
                json!({
                    "op": "add_bytes",
                    "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    "filename": filename,
                    "pin": true,
                }),
                ProviderTransfer::Bytes,
            )
            .await?;
        provider_response_ok(&ipfs_response, "content import_exact")?;
        let imported_cid = provider_response_cid(&ipfs_response).map_err(|err| {
            ProviderError::Provider(format!("content import_exact missing imported CID: {err}"))
        })?;
        if imported_cid != cid {
            let _ = self
                .invoke_provider(
                    &registry,
                    "ipfs",
                    "unpin",
                    json!({
                        "op": "unpin",
                        "cid": imported_cid,
                    }),
                    ProviderTransfer::Json,
                )
                .await;
            return Ok(provider_error(
                "cid_mismatch",
                "content import_exact produced a different CID; exact-CID import requires block-level compatible bytes",
            ));
        }

        let outcome = AvailabilityOutcome {
            provider: "ipfs-provider".to_string(),
            policy: "carrier_exact_import".to_string(),
            status: "local_pinned".to_string(),
            replicas: 1,
            reason: None,
            peer_selection: local_peer_selection_json(),
            quota: local_quota_json(),
            repair_worker: repair_worker_json(false),
            storage_market: local_storage_market_json(),
            repair_graph: local_repair_graph_json(),
            abuse_controls: local_abuse_controls_json(),
        };
        let object_did = request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let receipt = self.write_receipt(ReceiptInput {
            cid: cid.to_string(),
            object_did,
            publisher_did,
            provider: outcome.provider.clone(),
            policy: outcome.policy.clone(),
            status: outcome.status.clone(),
            replicas: outcome.replicas,
            peer_selection: outcome.peer_selection.clone(),
            quota: outcome.quota.clone(),
            repair_worker: outcome.repair_worker.clone(),
            storage_market: outcome.storage_market.clone(),
            repair_graph: outcome.repair_graph.clone(),
            abuse_controls: outcome.abuse_controls.clone(),
            accounting: content_accounting_json_with_storage_quota(
                "carrier_exact_import",
                ContentAccountingObservation {
                    files: Some(1),
                    bytes: Some(bytes.len() as u64),
                },
                outcome.replicas,
                storage_quota,
            ),
        })?;
        let repair_task = self.record_repair_task(&receipt, &outcome, requirements, false)?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "repair_task": repair_task,
            "receipt": receipt,
            "import": {
                "schema": "elastos.content.import-exact/v1",
                "method": "carrier_provider_stream",
                "bytes": bytes.len(),
                "verified_cid": true,
            }
        })))
    }

    async fn import_object(&self, request: &Value) -> Result<Value, ProviderError> {
        validate_import_object_invocation(request)?;
        let cid = request
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content import_object requires cid".into()))?;
        if !is_valid_cid(cid) {
            return Ok(provider_error(
                "invalid_cid",
                "content import_object requires a valid CID",
            ));
        }
        let files = request
            .get("files")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let (file_count, total_bytes) = validate_import_object_payload_bounds(&files)?;
        let requirements = AvailabilityRequirements::from_request(request);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = Some(self.effective_publisher_did(publisher_did.as_deref())?);
        let storage_quota = self.principal_storage_quota_for_request(
            publisher_did.as_deref().unwrap_or_default(),
            &requirements,
            Some(total_bytes as u64),
            Some(cid),
        )?;
        if storage_quota.get("status").and_then(|value| value.as_str()) == Some("quota_exceeded") {
            return Ok(provider_error(
                "storage_quota_exceeded",
                "content import_object exceeds the principal storage quota",
            ));
        }
        let object_kind = request
            .get("object_kind")
            .or_else(|| request.get("kind"))
            .and_then(|value| value.as_str())
            .unwrap_or("directory");
        let files = with_directory_object_manifest(
            files,
            object_kind,
            request.get("object_did").and_then(|value| value.as_str()),
            request
                .get("publisher_did")
                .and_then(|value| value.as_str()),
            request.get("links"),
        )?;
        let registry = self.registry()?;
        let ipfs_response = self
            .invoke_provider(
                &registry,
                "ipfs",
                "add_directory",
                json!({
                    "op": "add_directory",
                    "files": files,
                    "pin": true,
                }),
                ProviderTransfer::Json,
            )
            .await?;
        provider_response_ok(&ipfs_response, "content import_object")?;
        let imported_cid = provider_response_cid(&ipfs_response).map_err(|err| {
            ProviderError::Provider(format!("content import_object missing imported CID: {err}"))
        })?;
        if imported_cid != cid {
            let _ = self
                .invoke_provider(
                    &registry,
                    "ipfs",
                    "unpin",
                    json!({
                        "op": "unpin",
                        "cid": imported_cid,
                    }),
                    ProviderTransfer::Json,
                )
                .await;
            return Ok(provider_error(
                "cid_mismatch",
                "content import_object produced a different CID; exact object import requires matching manifest and bytes",
            ));
        }

        let outcome = AvailabilityOutcome {
            provider: "ipfs-provider".to_string(),
            policy: "carrier_object_import".to_string(),
            status: "local_pinned".to_string(),
            replicas: 1,
            reason: None,
            peer_selection: local_peer_selection_json(),
            quota: local_quota_json(),
            repair_worker: repair_worker_json(false),
            storage_market: local_storage_market_json(),
            repair_graph: local_repair_graph_json(),
            abuse_controls: local_abuse_controls_json(),
        };
        let object_did = request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let receipt = self.write_receipt(ReceiptInput {
            cid: cid.to_string(),
            object_did,
            publisher_did,
            provider: outcome.provider.clone(),
            policy: outcome.policy.clone(),
            status: outcome.status.clone(),
            replicas: outcome.replicas,
            peer_selection: outcome.peer_selection.clone(),
            quota: outcome.quota.clone(),
            repair_worker: outcome.repair_worker.clone(),
            storage_market: outcome.storage_market.clone(),
            repair_graph: outcome.repair_graph.clone(),
            abuse_controls: outcome.abuse_controls.clone(),
            accounting: content_accounting_json_with_storage_quota(
                "carrier_object_import",
                ContentAccountingObservation {
                    files: Some(file_count as u64),
                    bytes: Some(total_bytes as u64),
                },
                outcome.replicas,
                storage_quota,
            ),
        })?;
        let repair_task = self.record_repair_task(&receipt, &outcome, requirements, false)?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "repair_task": repair_task,
            "receipt": receipt,
            "import": {
                "schema": "elastos.content.import-object/v1",
                "method": "carrier_provider_object_manifest",
                "files": file_count,
                "bytes": total_bytes,
                "verified_cid": true,
            }
        })))
    }

    async fn unpublish(&self, request: &Value) -> Result<Value, ProviderError> {
        let cid = request
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content unpublish requires cid".into()))?;
        if !is_valid_cid(cid) {
            return Ok(provider_error(
                "invalid_cid",
                "content unpublish requires a valid CID",
            ));
        }

        let registry = self.registry()?;
        let ipfs_response = self
            .invoke_provider(
                &registry,
                "ipfs",
                "unpin",
                json!({
                    "op": "unpin",
                    "cid": cid,
                }),
                ProviderTransfer::Json,
            )
            .await?;
        provider_response_ok(&ipfs_response, "content unpublish")?;
        let previous_receipt = self.latest_receipt_for_cid(cid).transpose()?;
        let outcome = AvailabilityOutcome {
            provider: "ipfs-provider".to_string(),
            policy: "local_unpublish".to_string(),
            status: "local_unpinned".to_string(),
            replicas: 0,
            reason: None,
            peer_selection: local_peer_selection_json(),
            quota: local_quota_json(),
            repair_worker: repair_worker_json(false),
            storage_market: local_storage_market_json(),
            repair_graph: local_repair_graph_json(),
            abuse_controls: local_abuse_controls_json(),
        };
        let receipt = self.write_receipt(ReceiptInput {
            cid: cid.to_string(),
            object_did: request
                .get("object_did")
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .or_else(|| {
                    previous_receipt
                        .as_ref()
                        .and_then(|receipt| receipt.payload.object_did.clone())
                }),
            publisher_did: request
                .get("publisher_did")
                .and_then(|value| value.as_str())
                .map(str::to_string)
                .or_else(|| {
                    previous_receipt
                        .as_ref()
                        .map(|receipt| receipt.payload.publisher_did.clone())
                }),
            provider: outcome.provider.clone(),
            policy: outcome.policy.clone(),
            status: outcome.status.clone(),
            replicas: outcome.replicas,
            peer_selection: outcome.peer_selection.clone(),
            quota: outcome.quota.clone(),
            repair_worker: outcome.repair_worker.clone(),
            storage_market: outcome.storage_market.clone(),
            repair_graph: outcome.repair_graph.clone(),
            abuse_controls: outcome.abuse_controls.clone(),
            accounting: self.content_accounting_from_previous_or_unknown(
                cid,
                "local_unpublish",
                outcome.replicas,
            )?,
        })?;
        let repair_task = self.record_repair_task(
            &receipt,
            &outcome,
            AvailabilityRequirements::from_request(request),
            false,
        )?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "repair_task": repair_task,
            "receipt": receipt,
        })))
    }

    async fn ensure(&self, request: &Value) -> Result<Value, ProviderError> {
        self.pin_for_availability(request, "local_ensure_pin", "local_ensure_failed", false)
            .await
    }

    async fn repair(&self, request: &Value) -> Result<Value, ProviderError> {
        self.pin_for_availability(request, "local_repair_pin", "local_repair_failed", false)
            .await
    }

    async fn pin_for_availability(
        &self,
        request: &Value,
        success_policy: &str,
        failure_policy: &str,
        repair_worker_attempt: bool,
    ) -> Result<Value, ProviderError> {
        let cid = request
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content repair requires cid".into()))?;
        if !is_valid_cid(cid) {
            return Ok(provider_error(
                "invalid_cid",
                "content repair requires a valid CID",
            ));
        }

        let registry = self.registry()?;
        let ipfs_response = self
            .invoke_provider(
                &registry,
                "ipfs",
                "pin",
                json!({
                    "op": "pin",
                    "cid": cid,
                }),
                ProviderTransfer::Json,
            )
            .await?;

        let (status, policy, replicas, reason) = if ipfs_response
            .get("status")
            .and_then(|status| status.as_str())
            == Some("error")
        {
            (
                "repair_needed",
                failure_policy,
                0,
                ipfs_response
                    .get("message")
                    .and_then(|message| message.as_str())
                    .map(str::to_string),
            )
        } else {
            ("local_pinned", success_policy, 1, None)
        };

        let local_outcome = AvailabilityOutcome {
            provider: "ipfs-provider".to_string(),
            policy: policy.to_string(),
            status: status.to_string(),
            replicas,
            reason,
            peer_selection: local_peer_selection_json(),
            quota: local_quota_json(),
            repair_worker: repair_worker_json(status == "repair_needed"),
            storage_market: local_storage_market_json(),
            repair_graph: local_repair_graph_json(),
            abuse_controls: local_abuse_controls_json(),
        };
        let object_did = request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let accounting_observation = self
            .latest_receipt_for_cid(cid)
            .transpose()?
            .map(|receipt| content_accounting_observation_from_value(&receipt.payload.accounting))
            .unwrap_or_default();
        let outcome = if local_outcome.status == "local_pinned" {
            self.ensure_network_availability(
                &registry,
                cid,
                request,
                &local_outcome,
                AvailabilityRequestContext {
                    object_did: object_did.as_deref(),
                    publisher_did: publisher_did.as_deref(),
                    accounting_observation,
                },
            )
            .await?
            .unwrap_or(local_outcome)
        } else {
            local_outcome
        };

        let receipt = self.write_receipt(ReceiptInput {
            cid: cid.to_string(),
            object_did,
            publisher_did,
            provider: outcome.provider.clone(),
            policy: outcome.policy.clone(),
            status: outcome.status.clone(),
            replicas: outcome.replicas,
            peer_selection: outcome.peer_selection.clone(),
            quota: outcome.quota.clone(),
            repair_worker: outcome.repair_worker.clone(),
            storage_market: outcome.storage_market.clone(),
            repair_graph: outcome.repair_graph.clone(),
            abuse_controls: outcome.abuse_controls.clone(),
            accounting: self.content_accounting_from_previous_or_unknown(
                cid,
                if repair_worker_attempt {
                    "repair_worker"
                } else {
                    outcome.policy.as_str()
                },
                outcome.replicas,
            )?,
        })?;
        let repair_task = self.record_repair_task(
            &receipt,
            &outcome,
            AvailabilityRequirements::from_request(request),
            repair_worker_attempt,
        )?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "repair_task": repair_task,
            "receipt": receipt,
        })))
    }

    async fn ensure_network_availability(
        &self,
        registry: &ProviderRegistry,
        cid: &str,
        request: &Value,
        local: &AvailabilityOutcome,
        context: AvailabilityRequestContext<'_>,
    ) -> Result<Option<AvailabilityOutcome>, ProviderError> {
        let policy = request
            .get("availability_policy")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("network_default");
        let requirements = AvailabilityRequirements::from_request(request);
        let mut availability_request = json!({
            "op": "ensure",
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "policy": policy,
            "local": local.to_json(),
            "requirements": requirements.to_json(),
            "accounting": content_accounting_json(
                "availability_admission_estimate",
                context.accounting_observation,
                local.replicas,
            ),
        });
        if let Some(content_bytes) = context.accounting_observation.bytes {
            availability_request["estimated_content_bytes"] = Value::from(content_bytes);
        }
        if let Some(object_did) = context.object_did {
            availability_request["object_did"] = Value::String(object_did.to_string());
        }
        if let Some(publisher_did) = context.publisher_did {
            availability_request["publisher_did"] = Value::String(publisher_did.to_string());
        }

        match self
            .invoke_provider(
                registry,
                "availability",
                "ensure",
                availability_request,
                ProviderTransfer::Json,
            )
            .await
        {
            Ok(response) => Ok(Some(parse_availability_provider_response(
                &response,
                policy,
                local,
                &requirements,
            ))),
            Err(ProviderError::NoProvider(_)) => Ok(None),
            Err(err) => Ok(Some(AvailabilityOutcome::repair_needed(
                "availability-provider",
                policy,
                local.replicas,
                err.to_string(),
            ))),
        }
    }

    async fn run_repair_worker(&self, request: &Value) -> Result<Value, ProviderError> {
        validate_repair_worker_invocation(request)?;
        let force = request
            .get("force")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let include_healthy_check = request
            .get("include_healthy_check")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let limit = request
            .get("limit")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(REPAIR_WORKER_DEFAULT_LIMIT)
            .clamp(1, REPAIR_WORKER_MAX_LIMIT);
        let max_attempts = request
            .get("max_attempts")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(REPAIR_WORKER_DEFAULT_MAX_ATTEMPTS)
            .clamp(1, REPAIR_WORKER_MAX_ATTEMPTS_LIMIT);
        let failure_budget = request
            .get("failure_budget")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(REPAIR_WORKER_DEFAULT_FAILURE_BUDGET)
            .clamp(1, REPAIR_WORKER_MAX_FAILURE_BUDGET);
        let now = now_unix_secs();
        let tasks = self.latest_repair_tasks()?;
        let total_tasks = tasks.len();
        let mut checked = 0_u32;
        let mut repaired = 0_u32;
        let mut failed = 0_u32;
        let mut skipped = 0_u32;
        let mut exhausted_attempts_skipped = 0_u32;
        let mut throttled = false;
        let mut external_dispatches = 0_u32;
        let mut external_dispatch_accepted = 0_u32;
        let mut external_dispatch_failed = 0_u32;
        let mut results = Vec::new();

        for task in tasks {
            if results.len() >= limit {
                skipped = skipped.saturating_add(1);
                continue;
            }
            if !task.is_repair_candidate(include_healthy_check) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            if !force && !task.is_due(now) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            if task.attempts >= max_attempts {
                skipped = skipped.saturating_add(1);
                exhausted_attempts_skipped = exhausted_attempts_skipped.saturating_add(1);
                continue;
            }
            if failed >= failure_budget {
                skipped = skipped.saturating_add(1);
                throttled = true;
                continue;
            }

            checked = checked.saturating_add(1);
            let external_dispatch = if let Some(external_repair_fleet) = &self.external_repair_fleet
            {
                external_dispatches = external_dispatches.saturating_add(1);
                let dispatch_request = external_repair_fleet_dispatch_request(&task, now);
                let receipt = external_repair_fleet
                    .dispatch(&dispatch_request)
                    .await
                    .unwrap_or_else(external_repair_fleet_dispatch_failed);
                if receipt.get("accepted").and_then(|value| value.as_bool()) == Some(true) {
                    external_dispatch_accepted = external_dispatch_accepted.saturating_add(1);
                } else {
                    external_dispatch_failed = external_dispatch_failed.saturating_add(1);
                }
                Some(receipt)
            } else {
                None
            };
            let mut repair_request = json!({
                "op": "repair",
                "cid": task.cid,
                "availability_policy": task.policy,
                "availability_requirements": task.requirements,
            });
            if let Some(object_did) = task.object_did {
                repair_request["object_did"] = Value::String(object_did);
            }
            if let Some(publisher_did) = task.publisher_did {
                repair_request["publisher_did"] = Value::String(publisher_did);
            }

            match self
                .pin_for_availability(
                    &repair_request,
                    "local_repair_pin",
                    "local_repair_failed",
                    true,
                )
                .await
            {
                Ok(response)
                    if response.get("status").and_then(|value| value.as_str()) == Some("ok") =>
                {
                    let availability = response
                        .get("data")
                        .and_then(|data| data.get("availability"))
                        .cloned()
                        .unwrap_or_else(|| json!({"status": "unknown"}));
                    let status = availability
                        .get("status")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    if status != "repair_needed" {
                        repaired = repaired.saturating_add(1);
                    } else {
                        failed = failed.saturating_add(1);
                    }
                    results.push(json!({
                        "cid": repair_request["cid"],
                        "status": status,
                        "availability": availability,
                        "external_repair_fleet_dispatch": external_dispatch,
                    }));
                }
                Ok(response) => {
                    failed = failed.saturating_add(1);
                    results.push(json!({
                        "cid": repair_request["cid"],
                        "status": "failed",
                        "response": response,
                        "external_repair_fleet_dispatch": external_dispatch,
                    }));
                }
                Err(err) => {
                    failed = failed.saturating_add(1);
                    results.push(json!({
                        "cid": repair_request["cid"],
                        "status": "failed",
                        "message": err.to_string(),
                        "external_repair_fleet_dispatch": external_dispatch,
                    }));
                }
            }
        }

        Ok(provider_ok(json!({
            "schema": REPAIR_WORKER_RUN_SCHEMA,
            "total_tasks": total_tasks,
            "checked": checked,
            "repaired": repaired,
            "failed": failed,
            "skipped": skipped,
            "quota": {
                "policy": "content_repair_worker_guardrail",
                "scope": "content-availability",
                "limit": limit,
                "max_limit": REPAIR_WORKER_MAX_LIMIT,
                "max_attempts": max_attempts,
                "failure_budget": failure_budget,
            },
            "abuse_controls": {
                "schema": REPAIR_WORKER_ABUSE_CONTROLS_SCHEMA,
                "runtime_invocation_required": true,
                "app_visible": false,
                "force_due_override": force,
                "exhausted_attempts_skipped": exhausted_attempts_skipped,
                "throttled": throttled,
            },
            "network_abuse_policy": network_abuse_policy_run_json(
                checked,
                failed,
                exhausted_attempts_skipped,
                throttled,
            ),
            "repair_fleet": repair_fleet_run_json(
                checked,
                repaired,
                failed,
                skipped,
                exhausted_attempts_skipped,
                throttled,
            ),
            "external_repair_fleet_policy": external_repair_fleet_run_policy_json(
                ExternalRepairFleetRunSummary {
                    checked,
                    repaired,
                    failed,
                    skipped,
                    exhausted_attempts_skipped,
                    throttled,
                    external_dispatches,
                    external_dispatch_accepted,
                    external_dispatch_failed,
                },
                self.external_repair_fleet.as_ref(),
            ),
            "results": results,
        })))
    }

    async fn status(&self, request: &Value) -> Result<Value, ProviderError> {
        if let Some(cid) = request.get("cid").and_then(|cid| cid.as_str()) {
            if !is_valid_cid(cid) {
                return Ok(provider_error(
                    "invalid_cid",
                    "content status requires a valid CID",
                ));
            }
            if let Some(receipt) = self.latest_receipt_for_cid(cid) {
                let receipt = receipt?;
                let repair_task = self.latest_repair_task_for_cid(cid).transpose()?;
                let mut availability = json!({
                        "status": receipt.payload.status,
                        "provider": receipt.payload.provider,
                        "policy": receipt.payload.policy,
                        "replicas": receipt.payload.replicas,
                "peer_selection": receipt.payload.peer_selection,
                "quota": receipt.payload.quota,
                "federated_quota_ledger_policy": federated_quota_ledger_policy_from_quota_json(
                    &receipt.payload.quota,
                    false,
                ),
                "repair_worker": receipt.payload.repair_worker,
                "storage_market": receipt.payload.storage_market,
                "storage_settlement_policy": storage_settlement_policy_from_market_json(
                    &receipt.payload.storage_market,
                ),
                "storage_market_admission_policy": storage_market_admission_policy_from_market_json(
                    &receipt.payload.storage_market,
                ),
                "repair_graph": receipt.payload.repair_graph,
                "abuse_controls": receipt.payload.abuse_controls,
                "network_abuse_policy": network_abuse_policy_for_availability_json(
                    &receipt.payload.abuse_controls,
                    &receipt.payload.peer_selection,
                ),
                "accounting": receipt.payload.accounting,
                "checked_at": receipt.payload.checked_at,
                    });
                if let Some(repair_task) = repair_task {
                    availability["repair_task"] =
                        serde_json::to_value(repair_task).map_err(|err| {
                            ProviderError::Provider(format!(
                                "content repair task encode failed: {err}"
                            ))
                        })?;
                }
                if let Some(storage_accounting) = self
                    .latest_storage_accounting_entry_for_cid(cid)
                    .transpose()?
                {
                    availability["storage_accounting"] = serde_json::to_value(storage_accounting)
                        .map_err(|err| {
                        ProviderError::Provider(format!(
                            "content storage accounting encode failed: {err}"
                        ))
                    })?;
                }
                return Ok(provider_ok(json!({
                    "cid": receipt.payload.cid,
                    "uri": receipt.payload.uri,
                    "availability": availability,
                    "receipt": receipt,
                })));
            }
        }

        let emit_operator_alert = request
            .get("emit_operator_alert")
            .or_else(|| request.get("emit_operator_alerts"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        self.availability_dashboard(emit_operator_alert).await
    }

    fn write_receipt(
        &self,
        input: ReceiptInput,
    ) -> Result<SignedAvailabilityReceipt, ProviderError> {
        let (signing_key, default_did) = elastos_identity::load_or_create_did(&self.data_dir)
            .map_err(|err| {
                ProviderError::Provider(format!("content receipt signer unavailable: {err}"))
            })?;
        let publisher_did = input.publisher_did.unwrap_or(default_did);
        let receipt = AvailabilityReceipt {
            schema: AVAILABILITY_RECEIPT_SCHEMA.to_string(),
            cid: input.cid.clone(),
            uri: format!("elastos://{}", input.cid),
            object_did: input.object_did,
            publisher_did,
            provider: input.provider,
            policy: input.policy,
            status: input.status,
            replicas: input.replicas,
            peer_selection: input.peer_selection,
            quota: input.quota,
            repair_worker: input.repair_worker,
            storage_market: input.storage_market,
            repair_graph: input.repair_graph,
            abuse_controls: input.abuse_controls,
            accounting: input.accounting,
            checked_at: now_unix_secs(),
        };
        let payload_value = serde_json::to_value(&receipt).map_err(|err| {
            ProviderError::Provider(format!("content receipt encode failed: {err}"))
        })?;
        let payload = serde_json::to_string(&payload_value).map_err(|err| {
            ProviderError::Provider(format!("content receipt encode failed: {err}"))
        })?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &signing_key,
            AVAILABILITY_RECEIPT_DOMAIN,
            payload.as_bytes(),
        );
        let signed = SignedAvailabilityReceipt {
            payload: receipt,
            signature,
            signer_did,
        };
        append_jsonl(&self.receipts_path(), &signed)?;
        self.record_storage_accounting(&signed.payload)?;
        Ok(signed)
    }

    fn record_storage_accounting(
        &self,
        receipt: &AvailabilityReceipt,
    ) -> Result<ContentStorageAccountingEntry, ProviderError> {
        let entry = content_storage_accounting_entry_from_receipt(receipt);
        append_jsonl(&self.storage_accounting_path(), &entry)?;
        Ok(entry)
    }

    fn latest_receipt_for_cid(
        &self,
        cid: &str,
    ) -> Option<Result<SignedAvailabilityReceipt, ProviderError>> {
        match self.latest_receipts() {
            Ok(receipts) => receipts
                .into_iter()
                .find(|receipt| receipt.payload.cid == cid)
                .map(Ok),
            Err(err) => Some(Err(err)),
        }
    }

    fn latest_receipts(&self) -> Result<Vec<SignedAvailabilityReceipt>, ProviderError> {
        let path = self.receipts_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        let mut latest = BTreeMap::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let receipt: SignedAvailabilityReceipt =
                serde_json::from_str(&line).map_err(|err| {
                    ProviderError::Provider(format!("content receipt ledger decode failed: {err}"))
                })?;
            verify_signed_receipt(&receipt)?;
            latest.insert(receipt.payload.cid.clone(), receipt);
        }
        Ok(latest.into_values().collect())
    }

    fn latest_storage_accounting_entry_for_cid(
        &self,
        cid: &str,
    ) -> Option<Result<ContentStorageAccountingEntry, ProviderError>> {
        match self.latest_storage_accounting_entries() {
            Ok(entries) => entries.into_iter().find(|entry| entry.cid == cid).map(Ok),
            Err(err) => Some(Err(err)),
        }
    }

    fn latest_storage_accounting_entries(
        &self,
    ) -> Result<Vec<ContentStorageAccountingEntry>, ProviderError> {
        let path = self.storage_accounting_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        let mut latest = BTreeMap::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: ContentStorageAccountingEntry =
                serde_json::from_str(&line).map_err(|err| {
                    ProviderError::Provider(format!(
                        "content storage accounting ledger decode failed: {err}"
                    ))
                })?;
            if entry.schema != CONTENT_STORAGE_ACCOUNTING_ENTRY_SCHEMA {
                return Err(ProviderError::Provider(format!(
                    "content storage accounting ledger schema mismatch: {}",
                    entry.schema
                )));
            }
            latest.insert(entry.cid.clone(), entry);
        }
        Ok(latest.into_values().collect())
    }

    async fn availability_dashboard(
        &self,
        emit_operator_alert: bool,
    ) -> Result<Value, ProviderError> {
        let receipts = self.latest_receipts()?;
        let tasks = self.latest_repair_tasks()?;
        let storage_accounting_ledger = self.storage_accounting_ledger_status()?;
        let storage_accounting_entries = self.latest_storage_accounting_entries()?;
        let mut by_status: BTreeMap<String, u32> = BTreeMap::new();
        let mut by_provider: BTreeMap<String, u32> = BTreeMap::new();
        let mut by_quota_status: BTreeMap<String, u32> = BTreeMap::new();
        let mut total_replicas = 0_u32;
        let mut latest_checked_at = 0_u64;
        let mut quota_enforced = 0_u32;
        let mut quota_requirements_exceeded = 0_u32;
        let mut live_multi_peer_proofs = 0_u32;
        let mut remote_replicas = 0_u32;
        let mut remote_receipts = 0_u32;
        let mut verified_remote_receipts = 0_u32;
        let mut recent_remote_replicas = Vec::new();
        let mut peer_reputation_by_status: BTreeMap<String, u32> = BTreeMap::new();
        let mut peer_reputation_local_history_applied = 0_u32;
        let mut peer_reputation_federated = 0_u32;
        let mut accounted_objects = 0_u32;
        let mut accounted_files = 0_u64;
        let mut accounted_content_bytes = 0_u64;
        let mut accounted_replica_bytes = 0_u64;
        let mut storage_quota_enforced = 0_u32;
        let mut by_accounting_source: BTreeMap<String, u32> = BTreeMap::new();
        let mut abuse_controls_enforced = 0_u32;
        let mut abuse_controls_throttled = 0_u32;
        let mut abuse_attempted_operations = 0_u64;
        let mut abuse_failed_operations = 0_u64;
        let mut by_abuse_policy: BTreeMap<String, u32> = BTreeMap::new();
        for receipt in &receipts {
            *by_status.entry(receipt.payload.status.clone()).or_insert(0) += 1;
            *by_provider
                .entry(receipt.payload.provider.clone())
                .or_insert(0) += 1;
            total_replicas = total_replicas.saturating_add(receipt.payload.replicas);
            latest_checked_at = latest_checked_at.max(receipt.payload.checked_at);

            let quota_status = availability_quota_status(&receipt.payload.quota);
            *by_quota_status.entry(quota_status).or_insert(0) += 1;
            if receipt
                .payload
                .quota
                .get("enforced")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                quota_enforced = quota_enforced.saturating_add(1);
            }
            if receipt
                .payload
                .quota
                .get("requirements_exceed_quota")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                quota_requirements_exceeded = quota_requirements_exceeded.saturating_add(1);
            }
            if receipt
                .payload
                .peer_selection
                .get("live_multi_peer_proof")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                live_multi_peer_proofs = live_multi_peer_proofs.saturating_add(1);
            }
            let peer_reputation_policy =
                receipt.payload.peer_selection.get("peer_reputation_policy");
            let peer_attestation_exchange_policy = receipt
                .payload
                .peer_selection
                .get("peer_attestation_exchange_policy");
            let peer_reputation_status = peer_reputation_policy
                .and_then(|policy| policy.get("status"))
                .and_then(|value| value.as_str())
                .unwrap_or("not_reported")
                .to_string();
            *peer_reputation_by_status
                .entry(peer_reputation_status.clone())
                .or_insert(0) += 1;
            if peer_reputation_status == "local_history_applied" {
                peer_reputation_local_history_applied =
                    peer_reputation_local_history_applied.saturating_add(1);
            }
            if peer_reputation_policy
                .and_then(|policy| policy.get("federation"))
                .and_then(|federation| federation.get("configured"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                peer_reputation_federated = peer_reputation_federated.saturating_add(1);
            }
            for replica in receipt
                .payload
                .peer_selection
                .get("replicas")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                if replica.get("role").and_then(|value| value.as_str()) == Some("remote") {
                    remote_replicas = remote_replicas.saturating_add(1);
                    let remote_receipt = replica.get("remote_receipt");
                    recent_remote_replicas.push(json!({
                        "cid": receipt.payload.cid,
                        "node_did": replica
                            .get("node_did")
                            .cloned()
                            .unwrap_or(Value::Null),
                        "endpoint_id": replica
                            .get("endpoint_id")
                            .cloned()
                            .unwrap_or(Value::Null),
                        "score": replica
                            .get("score")
                            .cloned()
                            .unwrap_or(Value::Null),
                        "selection_reason": replica
                            .get("selection_reason")
                            .cloned()
                            .unwrap_or(Value::Null),
                        "peer_reputation_policy": peer_reputation_policy
                            .cloned()
                            .unwrap_or(Value::Null),
                        "peer_attestation_exchange_policy": peer_attestation_exchange_policy
                            .cloned()
                            .unwrap_or(Value::Null),
                        "local_reputation": replica
                            .get("local_reputation")
                            .cloned()
                            .unwrap_or(Value::Null),
                        "status": replica
                            .get("status")
                            .cloned()
                            .unwrap_or(Value::Null),
                        "checked_at": receipt.payload.checked_at,
                        "remote_receipt": {
                            "verified": remote_receipt
                                .and_then(|receipt| receipt.get("verified"))
                                .cloned()
                                .unwrap_or(Value::Bool(false)),
                            "status": remote_receipt
                                .and_then(|receipt| receipt.get("status"))
                                .cloned()
                                .unwrap_or(Value::Null),
                            "signer_did": remote_receipt
                                .and_then(|receipt| receipt.get("signer_did"))
                                .cloned()
                                .unwrap_or(Value::Null),
                            "quota_status": remote_receipt
                                .and_then(|receipt| receipt.get("quota"))
                                .and_then(|quota| quota.get("status"))
                                .cloned()
                                .unwrap_or(Value::Null),
                            "content_bytes": remote_receipt
                                .and_then(|receipt| receipt.get("accounting"))
                                .and_then(|accounting| accounting.get("content_bytes"))
                                .cloned()
                                .unwrap_or(Value::Null),
                            "abuse_controls": remote_receipt
                                .and_then(|receipt| receipt.get("abuse_controls"))
                                .cloned()
                                .unwrap_or(Value::Null),
                        },
                    }));
                }
                if let Some(remote_receipt) = replica.get("remote_receipt") {
                    remote_receipts = remote_receipts.saturating_add(1);
                    if remote_receipt
                        .get("verified")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                    {
                        verified_remote_receipts = verified_remote_receipts.saturating_add(1);
                    }
                }
            }

            let accounting = &receipt.payload.accounting;
            if accounting.get("schema").and_then(|value| value.as_str())
                == Some(CONTENT_ACCOUNTING_SCHEMA)
                && accounting
                    .get("observed")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            {
                accounted_objects = accounted_objects.saturating_add(1);
                if let Some(files) = accounting.get("files").and_then(|value| value.as_u64()) {
                    accounted_files = accounted_files.saturating_add(files);
                }
                if let Some(bytes) = accounting
                    .get("content_bytes")
                    .and_then(|value| value.as_u64())
                {
                    accounted_content_bytes = accounted_content_bytes.saturating_add(bytes);
                }
                if let Some(bytes) = accounting
                    .get("replica_bytes_estimate")
                    .and_then(|value| value.as_u64())
                {
                    accounted_replica_bytes = accounted_replica_bytes.saturating_add(bytes);
                }
                if accounting
                    .get("storage_quota")
                    .and_then(|quota| quota.get("enforced"))
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    storage_quota_enforced = storage_quota_enforced.saturating_add(1);
                }
                let source = accounting
                    .get("source")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                *by_accounting_source.entry(source).or_insert(0) += 1;
            }

            let abuse_controls = &receipt.payload.abuse_controls;
            if abuse_controls
                .get("schema")
                .and_then(|value| value.as_str())
                == Some(CONTENT_ABUSE_CONTROLS_SCHEMA)
            {
                if abuse_controls
                    .get("enforced")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    abuse_controls_enforced = abuse_controls_enforced.saturating_add(1);
                }
                if abuse_controls
                    .get("throttled")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    abuse_controls_throttled = abuse_controls_throttled.saturating_add(1);
                }
                if let Some(attempted) = abuse_controls
                    .get("attempted_operations")
                    .and_then(|value| value.as_u64())
                {
                    abuse_attempted_operations =
                        abuse_attempted_operations.saturating_add(attempted);
                }
                if let Some(failed) = abuse_controls
                    .get("failed_operations")
                    .and_then(|value| value.as_u64())
                {
                    abuse_failed_operations = abuse_failed_operations.saturating_add(failed);
                }
                let policy = abuse_controls
                    .get("policy")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                *by_abuse_policy.entry(policy).or_insert(0) += 1;
            }
        }

        let mut repair_status_counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut queued = 0_u32;
        let mut due = 0_u32;
        let mut healthy = 0_u32;
        let now = now_unix_secs();
        for task in &tasks {
            *repair_status_counts.entry(task.status.clone()).or_insert(0) += 1;
            if task.status == "queued" {
                queued = queued.saturating_add(1);
                if task.is_due(now) {
                    due = due.saturating_add(1);
                }
            }
            if task.status == "healthy" {
                healthy = healthy.saturating_add(1);
            }
        }

        let mut recent_tasks = tasks.clone();
        recent_tasks.sort_by(|a, b| {
            b.checked_at
                .cmp(&a.checked_at)
                .then_with(|| a.cid.cmp(&b.cid))
        });
        let recent_tasks = recent_tasks
            .iter()
            .take(10)
            .map(|task| {
                json!({
                    "cid": task.cid,
                    "status": task.status,
                    "attempts": task.attempts,
                    "checked_at": task.checked_at,
                    "next_check_after": task.next_check_after,
                    "due": task.is_due(now),
                })
            })
            .collect::<Vec<_>>();
        recent_remote_replicas.sort_by(|a, b| {
            b.get("checked_at")
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
                .cmp(
                    &a.get("checked_at")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0),
                )
                .then_with(|| {
                    a.get("cid")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .cmp(b.get("cid").and_then(|value| value.as_str()).unwrap_or(""))
                })
        });
        let recent_remote_replicas = recent_remote_replicas
            .into_iter()
            .take(AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT)
            .collect::<Vec<_>>();
        let recent_remote_replicas_truncated =
            remote_replicas as usize > recent_remote_replicas.len();

        let mut dashboard = json!({
            "schema": AVAILABILITY_DASHBOARD_SCHEMA,
            "provider": "content-provider",
            "objects": {
                "tracked": receipts.len(),
                "by_status": by_status,
                "by_provider": by_provider,
                "total_replicas": total_replicas,
                "latest_checked_at": latest_checked_at,
            },
            "quota": {
                "by_status": by_quota_status,
                "enforced": quota_enforced,
                "requirements_exceed_quota": quota_requirements_exceeded,
            },
            "federated_quota_ledger_policy": federated_quota_ledger_policy_status_json(
                &receipts,
                &storage_accounting_ledger,
                self.federated_quota_ledger_exchange.as_ref(),
            ),
            "proofs": {
                "live_multi_peer": live_multi_peer_proofs,
                "remote_replicas": remote_replicas,
                "remote_receipts": remote_receipts,
                "verified_remote_receipts": verified_remote_receipts,
                "recent_remote_replica_limit": AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT,
                "recent_remote_replicas_truncated": recent_remote_replicas_truncated,
                "recent_remote_replicas": recent_remote_replicas,
                "peer_reputation_policy": {
                    "schema": "elastos.carrier.peer-reputation/v1",
                    "policy": "local_runtime_reputation",
                    "scope": "content-availability",
                    "by_status": peer_reputation_by_status,
                    "local_history_applied": peer_reputation_local_history_applied,
                    "federated_policy_receipts": peer_reputation_federated,
                    "federation": {
                        "configured": peer_reputation_federated > 0,
                        "cross_runtime_reputation": peer_reputation_federated > 0,
                        "reason": if peer_reputation_federated > 0 {
                            "federated reputation receipts were observed"
                        } else {
                            "this branch records local Runtime success/failure reputation only"
                        },
                    },
                },
                "peer_attestation_exchange_policy": peer_attestation_exchange_policy_status_json(
                    &receipts,
                ),
            },
            "accounting": {
                "schema": CONTENT_ACCOUNTING_SCHEMA,
                "accounted_objects": accounted_objects,
                "accounted_files": accounted_files,
                "content_bytes": accounted_content_bytes,
                "replica_bytes_estimate": accounted_replica_bytes,
                "by_source": by_accounting_source,
                "storage_quota_enforced": storage_quota_enforced,
                "storage_quota_policy": "principal_ledger",
                "ledger": storage_accounting_ledger,
            },
            "abuse_controls": {
                "schema": CONTENT_ABUSE_CONTROLS_SCHEMA,
                "enforced": abuse_controls_enforced,
                "throttled": abuse_controls_throttled,
                "attempted_operations": abuse_attempted_operations,
                "failed_operations": abuse_failed_operations,
                "by_policy": by_abuse_policy,
            },
            "network_abuse_policy": network_abuse_policy_status_json(
                NetworkAbusePolicyStatusCounters {
                    tracked_objects: receipts.len(),
                    remote_replicas,
                    remote_receipts,
                    abuse_controls_enforced,
                    abuse_controls_throttled,
                    abuse_attempted_operations,
                    abuse_failed_operations,
                },
                self.federated_abuse_control_exchange.as_ref(),
            ),
            "storage_settlement_policy": storage_settlement_policy_status_json(
                &receipts,
                &storage_accounting_ledger,
            ),
            "storage_market_admission_policy": storage_market_admission_policy_status_json(
                &receipts,
                &storage_accounting_ledger,
                self.storage_market_admission.as_ref(),
            ),
            "repair": {
                "tracked_tasks": tasks.len(),
                "by_status": repair_status_counts,
                "queued": queued,
                "due": due,
                "healthy": healthy,
                "recent": recent_tasks,
            },
            "scheduler": {
                "manual_trigger": "elastos content repair-worker",
                "env": "ELASTOS_CONTENT_REPAIR_SCHEDULER",
                "enabled_by_default": false,
                "provider_invocation_required": true,
            },
            "repair_fleet": repair_fleet_status_json(&tasks, now),
            "external_repair_fleet_policy": external_repair_fleet_policy_json(
                &tasks,
                now,
                self.external_repair_fleet.as_ref(),
            ),
            "federated_operator_alerting_policy": federated_operator_alerting_policy_json(
                &receipts,
                &tasks,
                &storage_accounting_entries,
                &storage_accounting_ledger,
                now,
                self.operator_alert_sink.as_ref(),
                self.federated_operator_alert_exchange.as_ref(),
            ),
            "operator_dashboard": operator_dashboard_json(
                &receipts,
                &tasks,
                &storage_accounting_entries,
                &storage_accounting_ledger,
                now,
                ContentOperatorDashboardIntegrations {
                    operator_alert_sink: self.operator_alert_sink.as_ref(),
                    external_repair_fleet: self.external_repair_fleet.as_ref(),
                    federated_quota_ledger_exchange: self
                        .federated_quota_ledger_exchange
                        .as_ref(),
                    federated_operator_alert_exchange: self
                        .federated_operator_alert_exchange
                        .as_ref(),
                },
            ),
        });
        if emit_operator_alert {
            dashboard["operator_alert_delivery"] = self.emit_operator_alert(&dashboard).await?;
        }
        Ok(provider_ok(dashboard))
    }

    fn content_accounting_from_previous_or_unknown(
        &self,
        cid: &str,
        source: &str,
        replicas: u32,
    ) -> Result<Value, ProviderError> {
        let observation = self
            .latest_receipt_for_cid(cid)
            .transpose()?
            .map(|receipt| content_accounting_observation_from_value(&receipt.payload.accounting))
            .unwrap_or_default();
        Ok(content_accounting_json(source, observation, replicas))
    }

    fn principal_storage_quota_for_request(
        &self,
        principal_did: &str,
        requirements: &AvailabilityRequirements,
        incoming_content_bytes: Option<u64>,
        exclude_cid: Option<&str>,
    ) -> Result<Value, ProviderError> {
        let Some(max_content_bytes) = requirements.max_storage_bytes_per_principal else {
            return Ok(default_storage_quota_json());
        };
        let incoming_content_bytes = incoming_content_bytes.ok_or_else(|| {
            ProviderError::Provider(
                "content storage quota requires known incoming content bytes".to_string(),
            )
        })?;
        let active_content_bytes =
            self.principal_active_content_bytes(principal_did, exclude_cid)?;
        let projected_content_bytes = active_content_bytes.saturating_add(incoming_content_bytes);
        let quota_exceeded = projected_content_bytes > max_content_bytes;
        let status = if quota_exceeded {
            "quota_exceeded"
        } else {
            "within_quota"
        };
        Ok(json!({
            "schema": CONTENT_STORAGE_QUOTA_SCHEMA,
            "policy": "principal_storage_quota",
            "scope": "content-availability",
            "enforced": true,
            "status": status,
            "principal_did": principal_did,
            "active_content_bytes": active_content_bytes,
            "incoming_content_bytes": incoming_content_bytes,
            "projected_content_bytes": projected_content_bytes,
            "max_content_bytes": max_content_bytes,
            "federated_quota_ledger_policy": federated_quota_ledger_policy_json(
                "principal_storage_quota",
                status,
                true,
                false,
                true,
            ),
        }))
    }

    fn principal_active_content_bytes(
        &self,
        principal_did: &str,
        exclude_cid: Option<&str>,
    ) -> Result<u64, ProviderError> {
        Ok(self
            .latest_storage_accounting_entries()?
            .into_iter()
            .filter(|entry| {
                entry.principal_did == principal_did
                    && Some(entry.cid.as_str()) != exclude_cid
                    && entry.replicas > 0
                    && entry.status != "local_unpinned"
            })
            .filter_map(|entry| entry.content_bytes)
            .fold(0_u64, u64::saturating_add))
    }

    fn record_repair_task(
        &self,
        receipt: &SignedAvailabilityReceipt,
        outcome: &AvailabilityOutcome,
        requirements: AvailabilityRequirements,
        repair_worker_attempt: bool,
    ) -> Result<ContentRepairTask, ProviderError> {
        let previous_attempts = self
            .latest_repair_task_for_cid(&receipt.payload.cid)
            .transpose()?
            .map(|task| task.attempts)
            .unwrap_or(0);
        let attempts = previous_attempts.saturating_add(if repair_worker_attempt { 1 } else { 0 });
        let status = repair_task_status_for_availability(&outcome.status).to_string();
        let next_check_after =
            repair_task_next_check_after(&status, receipt.payload.checked_at, outcome);
        let mut repair_worker = outcome.repair_worker.clone();
        if !repair_worker.is_object() {
            repair_worker = repair_worker_json(status == "queued");
        }
        if let Some(metadata) = repair_worker.as_object_mut() {
            metadata.insert(
                "worker".to_string(),
                Value::String("content-provider".to_string()),
            );
            metadata.insert("status".to_string(), Value::String(status.clone()));
            metadata.insert(
                "scheduled".to_string(),
                Value::Bool(matches!(status.as_str(), "queued" | "healthy")),
            );
            metadata.insert(
                "next_check_after".to_string(),
                Value::from(next_check_after),
            );
        }
        let task = ContentRepairTask {
            schema: REPAIR_TASK_SCHEMA.to_string(),
            cid: receipt.payload.cid.clone(),
            uri: receipt.payload.uri.clone(),
            object_did: receipt.payload.object_did.clone(),
            publisher_did: Some(receipt.payload.publisher_did.clone()),
            policy: outcome.policy.clone(),
            status,
            reason: outcome.reason.clone(),
            attempts,
            requirements: requirements.to_json(),
            availability: outcome.to_json(),
            repair_worker,
            checked_at: receipt.payload.checked_at,
            next_check_after,
        };
        append_jsonl(&self.repair_tasks_path(), &task)?;
        Ok(task)
    }

    fn latest_repair_task_for_cid(
        &self,
        cid: &str,
    ) -> Option<Result<ContentRepairTask, ProviderError>> {
        match self.latest_repair_tasks() {
            Ok(tasks) => tasks.into_iter().find(|task| task.cid == cid).map(Ok),
            Err(err) => Some(Err(err)),
        }
    }

    fn latest_repair_tasks(&self) -> Result<Vec<ContentRepairTask>, ProviderError> {
        let path = self.repair_tasks_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        let mut latest = BTreeMap::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let task: ContentRepairTask = serde_json::from_str(&line).map_err(|err| {
                ProviderError::Provider(format!("content repair task ledger decode failed: {err}"))
            })?;
            latest.insert(task.cid.clone(), task);
        }
        Ok(latest.into_values().collect())
    }

    fn receipts_path(&self) -> PathBuf {
        self.data_dir
            .join("ElastOS")
            .join("SystemServices")
            .join("Content")
            .join("availability-receipts.jsonl")
    }

    fn repair_tasks_path(&self) -> PathBuf {
        self.data_dir
            .join("ElastOS")
            .join("SystemServices")
            .join("Content")
            .join("repair-tasks.jsonl")
    }

    fn storage_accounting_path(&self) -> PathBuf {
        self.data_dir
            .join("ElastOS")
            .join("SystemServices")
            .join("Content")
            .join("storage-accounting.jsonl")
    }

    fn operator_alert_receipts_path(&self) -> PathBuf {
        self.data_dir
            .join("ElastOS")
            .join("SystemServices")
            .join("Content")
            .join("operator-alert-receipts.jsonl")
    }

    async fn emit_operator_alert(&self, dashboard: &Value) -> Result<Value, ProviderError> {
        let emitted_at = now_unix_secs();
        let alert = operator_alert_payload(dashboard, emitted_at);
        let local_delivery = match &self.operator_alert_sink {
            Some(sink) => sink.deliver(&alert).await,
            None => Ok(json!({
                "configured": false,
                "delivered": false,
                "status": "not_configured",
                "reason": "no content operator alert sink is configured",
            })),
        };
        let delivery = match local_delivery {
            Ok(delivery) => delivery,
            Err(err) => json!({
                "configured": true,
                "delivered": false,
                "status": "failed",
                "reason": err,
            }),
        };
        let federated_exchange = match &self.federated_operator_alert_exchange {
            Some(exchange) => {
                let request = federated_operator_alert_exchange_request(&alert, emitted_at);
                match exchange.exchange(&request).await {
                    Ok(receipt) => receipt,
                    Err(err) => json!({
                        "schema": CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_RECEIPT_SCHEMA,
                        "provider": "content-provider",
                        "scope": "content-availability",
                        "configured": true,
                        "delivered": false,
                        "accepted": false,
                        "status": "failed",
                        "reason": err,
                        "exchange": exchange.redacted_status_json(),
                        "credential_exposed": false,
                    }),
                }
            }
            None => json!({
                "schema": CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_RECEIPT_SCHEMA,
                "provider": "content-provider",
                "scope": "content-availability",
                "configured": false,
                "delivered": false,
                "accepted": false,
                "status": "not_configured",
                "reason": "no federated operator alert exchange is configured",
            }),
        };
        let receipt = json!({
            "schema": CONTENT_OPERATOR_ALERT_RECEIPT_SCHEMA,
            "provider": "content-provider",
            "scope": "content-availability",
            "emitted_at": emitted_at,
            "requested": true,
            "alert": alert,
            "delivery": delivery,
            "federated_exchange": federated_exchange,
        });
        append_jsonl(&self.operator_alert_receipts_path(), &receipt)?;
        Ok(receipt)
    }

    fn sign_admission_receipt(&self, admission: &Value) -> Result<Value, ProviderError> {
        let (signing_key, _) =
            elastos_identity::load_or_create_did(&self.data_dir).map_err(|err| {
                ProviderError::Provider(format!(
                    "content admission receipt signer unavailable: {err}"
                ))
            })?;
        let payload = serde_json::to_string(admission).map_err(|err| {
            ProviderError::Provider(format!("content admission receipt encode failed: {err}"))
        })?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &signing_key,
            CONTENT_ADMISSION_DOMAIN,
            payload.as_bytes(),
        );
        Ok(json!({
            "payload": admission,
            "signature": signature,
            "signer_did": signer_did,
        }))
    }

    fn sign_federated_quota_ledger_exchange_request(
        &self,
        local_admission: &Value,
        request: &Value,
    ) -> Result<Value, ProviderError> {
        let (signing_key, _) =
            elastos_identity::load_or_create_did(&self.data_dir).map_err(|err| {
                ProviderError::Provider(format!(
                    "content federated quota-ledger exchange signer unavailable: {err}"
                ))
            })?;
        let payload = federated_quota_ledger_exchange_request(local_admission, request);
        let canonical = serde_json::to_string(&payload).map_err(|err| {
            ProviderError::Provider(format!(
                "content federated quota-ledger exchange request encode failed: {err}"
            ))
        })?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &signing_key,
            CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_DOMAIN,
            canonical.as_bytes(),
        );
        Ok(json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        }))
    }

    fn sign_federated_abuse_control_exchange_request(
        &self,
        local_admission: &Value,
        request: &Value,
    ) -> Result<Value, ProviderError> {
        let (signing_key, _) =
            elastos_identity::load_or_create_did(&self.data_dir).map_err(|err| {
                ProviderError::Provider(format!(
                    "content federated abuse-control exchange signer unavailable: {err}"
                ))
            })?;
        let payload = federated_abuse_control_exchange_request(local_admission, request);
        let canonical = serde_json::to_string(&payload).map_err(|err| {
            ProviderError::Provider(format!(
                "content federated abuse-control exchange request encode failed: {err}"
            ))
        })?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &signing_key,
            CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_DOMAIN,
            canonical.as_bytes(),
        );
        Ok(json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        }))
    }

    fn storage_accounting_ledger_status(&self) -> Result<Value, ProviderError> {
        #[derive(Default)]
        struct PrincipalTotals {
            tracked_objects: u32,
            active_objects: u32,
            files: u64,
            content_bytes: u64,
            replica_bytes_estimate: u64,
            quota_enforced: u32,
            latest_recorded_at: u64,
            by_status: BTreeMap<String, u32>,
            by_quota_status: BTreeMap<String, u32>,
        }

        let entries = self.latest_storage_accounting_entries()?;
        let mut by_principal: BTreeMap<String, PrincipalTotals> = BTreeMap::new();
        let mut active_objects = 0_u32;
        let mut active_principals = BTreeSet::new();
        let mut content_bytes = 0_u64;
        let mut replica_bytes_estimate = 0_u64;
        let mut quota_enforced = 0_u32;
        let mut latest_recorded_at = 0_u64;

        for entry in &entries {
            let active = entry.replicas > 0 && entry.status != "local_unpinned";
            let quota_status = availability_quota_status(&entry.quota);
            let storage_quota_enforced = entry
                .storage_quota
                .get("enforced")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
                || entry
                    .quota
                    .get("enforced")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);

            let principal = by_principal.entry(entry.principal_did.clone()).or_default();
            principal.tracked_objects = principal.tracked_objects.saturating_add(1);
            *principal.by_status.entry(entry.status.clone()).or_insert(0) += 1;
            *principal.by_quota_status.entry(quota_status).or_insert(0) += 1;
            principal.latest_recorded_at = principal.latest_recorded_at.max(entry.recorded_at);

            if storage_quota_enforced {
                quota_enforced = quota_enforced.saturating_add(1);
                principal.quota_enforced = principal.quota_enforced.saturating_add(1);
            }

            if active {
                active_objects = active_objects.saturating_add(1);
                active_principals.insert(entry.principal_did.clone());
                principal.active_objects = principal.active_objects.saturating_add(1);
                if let Some(files) = entry.files {
                    principal.files = principal.files.saturating_add(files);
                }
                if let Some(bytes) = entry.content_bytes {
                    content_bytes = content_bytes.saturating_add(bytes);
                    principal.content_bytes = principal.content_bytes.saturating_add(bytes);
                }
                if let Some(bytes) = entry.replica_bytes_estimate {
                    replica_bytes_estimate = replica_bytes_estimate.saturating_add(bytes);
                    principal.replica_bytes_estimate =
                        principal.replica_bytes_estimate.saturating_add(bytes);
                }
            }

            latest_recorded_at = latest_recorded_at.max(entry.recorded_at);
        }

        let by_principal = by_principal
            .into_iter()
            .map(|(principal_did, totals)| {
                (
                    principal_did,
                    json!({
                        "tracked_objects": totals.tracked_objects,
                        "active_objects": totals.active_objects,
                        "files": totals.files,
                        "content_bytes": totals.content_bytes,
                        "replica_bytes_estimate": totals.replica_bytes_estimate,
                        "quota_enforced": totals.quota_enforced,
                        "by_status": totals.by_status,
                        "by_quota_status": totals.by_quota_status,
                        "latest_recorded_at": totals.latest_recorded_at,
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(json!({
            "schema": CONTENT_STORAGE_ACCOUNTING_LEDGER_SCHEMA,
            "durable": true,
            "source": "signed_availability_receipts",
            "tracked_objects": entries.len(),
            "active_objects": active_objects,
            "tracked_principals": by_principal.len(),
            "active_principals": active_principals.len(),
            "content_bytes": content_bytes,
            "replica_bytes_estimate": replica_bytes_estimate,
            "quota_enforced": quota_enforced,
            "latest_recorded_at": latest_recorded_at,
            "by_principal": by_principal,
            "market_policy": {
                "schema": "elastos.content.storage-market/v1",
                "mode": "provider_receipt_accounting",
                "status": if quota_enforced > 0 {
                    "quota_policy_recorded_no_settlement"
                } else {
                    "accounting_recorded_no_settlement"
                },
                "settlement": "not_configured",
                "escrow": "not_configured",
                "admission_policy": storage_market_admission_policy_json(
                    "provider_receipt_accounting",
                    if quota_enforced > 0 {
                        "quota_policy_recorded_no_settlement"
                    } else {
                        "accounting_recorded_no_settlement"
                    },
                    quota_enforced > 0,
                    false,
                    false,
                ),
                "settlement_policy": storage_settlement_policy_json(
                    "provider_receipt_accounting",
                    if quota_enforced > 0 {
                        "quota_policy_recorded_no_settlement"
                    } else {
                        "accounting_recorded_no_settlement"
                    },
                    quota_enforced > 0,
                ),
                "next": "External pricing, escrow/settlement, storage-market admission, and cross-peer SLA policy remain production-network work."
            },
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityReceipt {
    pub schema: String,
    pub cid: String,
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_did: Option<String>,
    pub publisher_did: String,
    pub provider: String,
    pub policy: String,
    pub status: String,
    pub replicas: u32,
    pub peer_selection: Value,
    pub quota: Value,
    pub repair_worker: Value,
    #[serde(
        default = "default_content_storage_market_json",
        skip_serializing_if = "is_default_content_storage_market_json"
    )]
    pub storage_market: Value,
    #[serde(
        default = "default_content_repair_graph_json",
        skip_serializing_if = "is_default_content_repair_graph_json"
    )]
    pub repair_graph: Value,
    #[serde(default = "default_content_abuse_controls_json")]
    pub abuse_controls: Value,
    #[serde(default = "default_content_accounting_json")]
    pub accounting: Value,
    pub checked_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedAvailabilityReceipt {
    pub payload: AvailabilityReceipt,
    pub signature: String,
    pub signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContentRepairTask {
    schema: String,
    cid: String,
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publisher_did: Option<String>,
    policy: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    attempts: u32,
    requirements: Value,
    availability: Value,
    repair_worker: Value,
    checked_at: u64,
    next_check_after: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContentStorageAccountingEntry {
    schema: String,
    cid: String,
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_did: Option<String>,
    principal_did: String,
    provider: String,
    policy: String,
    status: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_bytes: Option<u64>,
    replicas: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    replica_bytes_estimate: Option<u64>,
    quota: Value,
    storage_quota: Value,
    recorded_at: u64,
}

impl ContentRepairTask {
    fn is_repair_candidate(&self, include_healthy_check: bool) -> bool {
        matches!(self.status.as_str(), "queued" | "repair_needed")
            || (include_healthy_check && self.status == "healthy")
    }

    fn is_due(&self, now: u64) -> bool {
        self.next_check_after == 0 || self.next_check_after <= now
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentObjectManifest {
    pub schema: String,
    pub kind: String,
    pub content_digest: String,
    pub files: Vec<ContentObjectFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ContentObjectLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher_did: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentObjectFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentObjectLink {
    pub rel: String,
    pub cid: String,
}

struct ReceiptInput {
    cid: String,
    object_did: Option<String>,
    publisher_did: Option<String>,
    provider: String,
    policy: String,
    status: String,
    replicas: u32,
    peer_selection: Value,
    quota: Value,
    repair_worker: Value,
    storage_market: Value,
    repair_graph: Value,
    abuse_controls: Value,
    accounting: Value,
}

struct AvailabilityRequestContext<'a> {
    object_did: Option<&'a str>,
    publisher_did: Option<&'a str>,
    accounting_observation: ContentAccountingObservation,
}

#[derive(Debug, Clone)]
struct AvailabilityRequirements {
    min_replicas: u32,
    max_replicas: Option<u32>,
    require_live_multi_peer_proof: bool,
    max_storage_bytes_per_principal: Option<u64>,
    repair_graph_kind: Option<String>,
}

impl AvailabilityRequirements {
    fn from_request(request: &Value) -> Self {
        let requirements = request
            .get("availability_requirements")
            .or_else(|| request.get("replication_requirements"));
        let min_replicas = requirements
            .and_then(|value| value.get("min_replicas"))
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(1)
            .max(1);
        let max_replicas = requirements
            .and_then(|value| value.get("max_replicas"))
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0);
        let require_live_multi_peer_proof = requirements
            .and_then(|value| value.get("require_live_multi_peer_proof"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let max_storage_bytes_per_principal = requirements
            .and_then(|value| {
                value
                    .get("max_storage_bytes_per_principal")
                    .or_else(|| value.get("max_principal_storage_bytes"))
                    .or_else(|| value.get("max_storage_bytes"))
                    .or_else(|| value.get("storage_quota_bytes"))
            })
            .and_then(|value| value.as_u64())
            .filter(|value| *value > 0);
        let repair_graph_kind = requirements
            .and_then(|value| {
                value
                    .get("repair_graph_kind")
                    .or_else(|| value.get("content_graph_kind"))
                    .or_else(|| value.get("graph_kind"))
                    .or_else(|| {
                        value
                            .get("repair_graph")
                            .and_then(|graph| graph.get("kind"))
                    })
                    .or_else(|| {
                        value
                            .get("content_graph")
                            .and_then(|graph| graph.get("kind"))
                    })
            })
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        Self {
            min_replicas,
            max_replicas,
            require_live_multi_peer_proof,
            max_storage_bytes_per_principal,
            repair_graph_kind,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "min_replicas": self.min_replicas,
            "max_replicas": self.max_replicas,
            "require_live_multi_peer_proof": self.require_live_multi_peer_proof,
            "max_storage_bytes_per_principal": self.max_storage_bytes_per_principal,
            "repair_graph_kind": self.repair_graph_kind,
        })
    }
}

impl Default for AvailabilityRequirements {
    fn default() -> Self {
        Self {
            min_replicas: 1,
            max_replicas: None,
            require_live_multi_peer_proof: false,
            max_storage_bytes_per_principal: None,
            repair_graph_kind: None,
        }
    }
}

#[derive(Debug, Clone)]
struct AvailabilityOutcome {
    provider: String,
    policy: String,
    status: String,
    replicas: u32,
    reason: Option<String>,
    peer_selection: Value,
    quota: Value,
    repair_worker: Value,
    storage_market: Value,
    repair_graph: Value,
    abuse_controls: Value,
}

impl AvailabilityOutcome {
    fn local_publish(pin: bool) -> Self {
        if pin {
            Self {
                provider: "ipfs-provider".to_string(),
                policy: "local_pin".to_string(),
                status: "local_pinned".to_string(),
                replicas: 1,
                reason: None,
                peer_selection: local_peer_selection_json(),
                quota: local_quota_json(),
                repair_worker: repair_worker_json(false),
                storage_market: local_storage_market_json(),
                repair_graph: local_repair_graph_json(),
                abuse_controls: local_abuse_controls_json(),
            }
        } else {
            Self {
                provider: "ipfs-provider".to_string(),
                policy: "local_add".to_string(),
                status: "local_unpinned".to_string(),
                replicas: 0,
                reason: None,
                peer_selection: local_peer_selection_json(),
                quota: local_quota_json(),
                repair_worker: repair_worker_json(false),
                storage_market: local_storage_market_json(),
                repair_graph: local_repair_graph_json(),
                abuse_controls: local_abuse_controls_json(),
            }
        }
    }

    fn repair_needed(provider: &str, policy: &str, replicas: u32, reason: String) -> Self {
        Self {
            provider: provider.to_string(),
            policy: policy.to_string(),
            status: "repair_needed".to_string(),
            replicas,
            reason: Some(reason),
            peer_selection: local_peer_selection_json(),
            quota: local_quota_json(),
            repair_worker: repair_worker_json(true),
            storage_market: local_storage_market_json(),
            repair_graph: local_repair_graph_json(),
            abuse_controls: local_abuse_controls_json(),
        }
    }

    fn to_json(&self) -> Value {
        let mut availability = json!({
            "status": self.status,
            "provider": self.provider,
            "policy": self.policy,
            "replicas": self.replicas,
            "peer_selection": self.peer_selection,
            "quota": self.quota,
            "federated_quota_ledger_policy": federated_quota_ledger_policy_from_quota_json(
                &self.quota,
                false,
            ),
            "repair_worker": self.repair_worker,
            "storage_market": self.storage_market,
            "storage_settlement_policy": storage_settlement_policy_from_market_json(
                &self.storage_market,
            ),
            "storage_market_admission_policy": storage_market_admission_policy_from_market_json(
                &self.storage_market,
            ),
            "repair_graph": self.repair_graph,
            "abuse_controls": self.abuse_controls,
            "network_abuse_policy": network_abuse_policy_for_availability_json(
                &self.abuse_controls,
                &self.peer_selection,
            ),
        });
        if let Some(reason) = &self.reason {
            availability["reason"] = Value::String(reason.clone());
        }
        availability
    }
}

fn repair_task_status_for_availability(status: &str) -> &'static str {
    match status {
        "repair_needed" => "queued",
        "network_available" | "carrier_announced" => "healthy",
        "local_unpinned" => "retired",
        "local_pinned" => "local_only",
        _ => "observed",
    }
}

fn repair_task_next_check_after(
    task_status: &str,
    checked_at: u64,
    outcome: &AvailabilityOutcome,
) -> u64 {
    match task_status {
        "queued" => checked_at.saturating_add(REPAIR_RETRY_DELAY_SECS),
        "healthy" => checked_at.saturating_add(REPAIR_HEALTH_CHECK_DELAY_SECS),
        "local_only" if outcome.status == "local_pinned" => 0,
        _ => 0,
    }
}

fn network_abuse_policy_for_availability_json(
    abuse_controls: &Value,
    peer_selection: &Value,
) -> Value {
    let enforced = abuse_controls
        .get("enforced")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let throttled = abuse_controls
        .get("throttled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let attempted_operations = abuse_controls
        .get("attempted_operations")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let failed_operations = abuse_controls
        .get("failed_operations")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let remote_replicas = peer_selection
        .get("replicas")
        .and_then(|value| value.as_array())
        .map(|replicas| {
            replicas
                .iter()
                .filter(|replica| {
                    replica.get("role").and_then(|value| value.as_str()) == Some("remote")
                })
                .count()
        })
        .unwrap_or(0);
    json!({
        "schema": CONTENT_NETWORK_ABUSE_POLICY_SCHEMA,
        "policy": "provider_plane_local_guardrails",
        "scope": "content-availability",
        "status": if enforced {
            "local_provider_guardrails_recorded"
        } else {
            "local_backend_no_network_guardrail"
        },
        "authority": {
            "provider": "content-provider",
            "runtime_invocation_required": true,
            "app_visible": false,
        },
        "receipt": {
            "abuse_controls_policy": abuse_controls
                .get("policy")
                .cloned()
                .unwrap_or(Value::String("not_reported".to_string())),
            "enforced": enforced,
            "throttled": throttled,
            "attempted_operations": attempted_operations,
            "failed_operations": failed_operations,
            "remote_replicas": remote_replicas,
        },
        "local_guardrails": {
            "signed_availability_receipts": true,
            "carrier_provider_candidate_cap": enforced,
            "carrier_provider_admission_preflight": enforced && remote_replicas > 0,
            "repair_worker_attempt_budget": true,
            "provider_invocation_required": true,
        },
        "network_federation": {
            "configured": false,
            "cross_peer_rate_limit": false,
            "federated_banlist": false,
            "federated_abuse_ledger": false,
            "reason": "this receipt exposes provider-owned local guardrails; production network-wide throttles, banlists, and abuse ledgers require a configured policy plane",
        },
    })
}

struct NetworkAbusePolicyStatusCounters {
    tracked_objects: usize,
    remote_replicas: u32,
    remote_receipts: u32,
    abuse_controls_enforced: u32,
    abuse_controls_throttled: u32,
    abuse_attempted_operations: u64,
    abuse_failed_operations: u64,
}

fn network_abuse_policy_status_json(
    counters: NetworkAbusePolicyStatusCounters,
    federated_abuse_control_exchange: Option<&ContentFederatedAbuseControlExchangeClient>,
) -> Value {
    json!({
        "schema": CONTENT_NETWORK_ABUSE_POLICY_SCHEMA,
        "policy": "provider_plane_local_guardrails",
        "scope": "content-availability",
        "status": if federated_abuse_control_exchange.is_some() {
            "configured_federated_abuse_control_exchange"
        } else if counters.abuse_controls_enforced > 0 || counters.remote_replicas > 0 {
            "local_guardrails_recorded_no_federated_throttle"
        } else {
            "local_only_no_network_activity"
        },
        "authority": {
            "provider": "content-provider",
            "runtime_invocation_required": true,
            "app_visible": false,
        },
        "local_guardrails": {
            "signed_availability_receipts": true,
            "carrier_provider_candidate_cap": true,
            "carrier_provider_admission_preflight": true,
            "repair_worker_attempt_budget": true,
            "repair_worker_failure_budget": true,
            "provider_invocation_required": true,
        },
        "counters": {
            "tracked_objects": counters.tracked_objects,
            "remote_replicas": counters.remote_replicas,
            "remote_receipts": counters.remote_receipts,
            "guardrail_receipts": counters.abuse_controls_enforced,
            "throttled_receipts": counters.abuse_controls_throttled,
            "attempted_provider_operations": counters.abuse_attempted_operations,
            "failed_provider_operations": counters.abuse_failed_operations,
        },
        "network_federation": {
            "configured": federated_abuse_control_exchange.is_some(),
            "cross_peer_rate_limit": federated_abuse_control_exchange.is_some(),
            "federated_banlist": false,
            "federated_abuse_ledger": federated_abuse_control_exchange.is_some(),
            "exchange_client": federated_abuse_control_exchange
                .map(ContentFederatedAbuseControlExchangeClient::redacted_status_json)
                .unwrap_or_else(|| {
                    json!({
                        "configured": false,
                        "delivery": "not_configured",
                        "authorization_configured": false,
                    })
                }),
            "reason": if federated_abuse_control_exchange.is_some() {
                "configured federated abuse-control exchange is enforced by content/admission before remote bytes or DAG repair data move; production banlists and network-wide abuse policy remain external infrastructure"
            } else {
                "cross-peer throttles, banlists, and abuse ledgers are production-network policy, not app-visible Library authority"
            },
        },
    })
}

fn network_abuse_policy_run_json(
    checked: u32,
    failed: u32,
    exhausted_attempts_skipped: u32,
    throttled: bool,
) -> Value {
    json!({
        "schema": CONTENT_NETWORK_ABUSE_POLICY_SCHEMA,
        "policy": "provider_plane_local_guardrails",
        "scope": "content-availability",
        "status": if throttled || exhausted_attempts_skipped > 0 {
            "local_worker_throttled"
        } else {
            "local_worker_within_budget"
        },
        "authority": {
            "provider": "content-provider",
            "runtime_invocation_required": true,
            "app_visible": false,
        },
        "local_guardrails": {
            "repair_worker_attempt_budget": true,
            "repair_worker_failure_budget": true,
            "force_due_override_audited": true,
        },
        "run": {
            "checked": checked,
            "failed": failed,
            "exhausted_attempts_skipped": exhausted_attempts_skipped,
            "throttled": throttled,
        },
        "network_federation": {
            "configured": false,
            "cross_peer_rate_limit": false,
            "federated_banlist": false,
            "federated_abuse_ledger": false,
        },
    })
}

fn peer_attestation_exchange_policy_status_json(receipts: &[SignedAvailabilityReceipt]) -> Value {
    let mut by_status: BTreeMap<String, u32> = BTreeMap::new();
    let mut policy_receipts = 0_u32;
    let mut configured_receipts = 0_u32;
    let mut remote_provider_proofs = 0_u64;
    let mut verified_remote_content_receipts = 0_u64;

    for receipt in receipts {
        let policy = receipt
            .payload
            .peer_selection
            .get("peer_attestation_exchange_policy");
        if policy.is_some() {
            policy_receipts = policy_receipts.saturating_add(1);
        }
        let status = policy
            .and_then(|policy| policy.get("status"))
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported")
            .to_string();
        *by_status.entry(status).or_insert(0) += 1;
        if policy
            .and_then(|policy| policy.get("attestation_exchange"))
            .and_then(|exchange| exchange.get("configured"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            configured_receipts = configured_receipts.saturating_add(1);
        }
        if let Some(proofs) = policy
            .and_then(|policy| policy.get("local_proof"))
            .and_then(|proof| proof.get("remote_provider_proofs"))
            .and_then(|value| value.as_u64())
        {
            remote_provider_proofs = remote_provider_proofs.saturating_add(proofs);
        }
        if let Some(verified) = policy
            .and_then(|policy| policy.get("local_proof"))
            .and_then(|proof| proof.get("verified_remote_content_receipts"))
            .and_then(|value| value.as_u64())
        {
            verified_remote_content_receipts =
                verified_remote_content_receipts.saturating_add(verified);
        }
    }

    json!({
        "schema": CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA,
        "policy": "provider_receipt_attestation_exchange_status",
        "scope": "content-availability",
        "status": if configured_receipts > 0 {
            "attestation_exchange_observed"
        } else if remote_provider_proofs > 0 {
            "live_peer_proof_without_attestation_exchange"
        } else {
            "attestation_exchange_not_configured"
        },
        "receipt_count": receipts.len(),
        "policy_receipts": policy_receipts,
        "by_status": by_status,
        "local_proof": {
            "signed_availability_announcements": true,
            "remote_provider_proofs": remote_provider_proofs,
            "verified_remote_content_receipts": verified_remote_content_receipts,
            "local_runtime_reputation": true,
        },
        "attestation_exchange": {
            "configured": configured_receipts > 0,
            "signed_reputation_receipts": configured_receipts > 0,
            "third_party_attestations": false,
            "cross_runtime_trust_policy": false,
            "revocation": "not_configured",
            "reason": if configured_receipts > 0 {
                "one or more receipts reported cross-runtime reputation attestations"
            } else {
                "no signed cross-runtime reputation attestation exchange is configured"
            },
        },
    })
}

#[derive(Clone, Copy)]
struct ContentOperatorDashboardIntegrations<'a> {
    operator_alert_sink: Option<&'a ContentOperatorAlertSink>,
    external_repair_fleet: Option<&'a ContentExternalRepairFleetClient>,
    federated_quota_ledger_exchange: Option<&'a ContentFederatedQuotaLedgerExchangeClient>,
    federated_operator_alert_exchange: Option<&'a ContentFederatedOperatorAlertExchangeClient>,
}

fn operator_dashboard_json(
    receipts: &[SignedAvailabilityReceipt],
    tasks: &[ContentRepairTask],
    storage_entries: &[ContentStorageAccountingEntry],
    storage_ledger: &Value,
    now: u64,
    integrations: ContentOperatorDashboardIntegrations<'_>,
) -> Value {
    #[derive(Default)]
    struct PrincipalPressure {
        active_objects: u32,
        files: u64,
        content_bytes: u64,
        replica_bytes_estimate: u64,
        quota_enforced: u32,
        latest_recorded_at: u64,
    }

    let mut principals: BTreeMap<String, PrincipalPressure> = BTreeMap::new();
    let mut quota_exceeded_records = 0_u32;
    for entry in storage_entries {
        let active = entry.replicas > 0 && entry.status != "local_unpinned";
        if entry
            .storage_quota
            .get("status")
            .or_else(|| entry.quota.get("status"))
            .and_then(|value| value.as_str())
            == Some("quota_exceeded")
        {
            quota_exceeded_records = quota_exceeded_records.saturating_add(1);
        }
        if !active {
            continue;
        }

        let principal = principals.entry(entry.principal_did.clone()).or_default();
        principal.active_objects = principal.active_objects.saturating_add(1);
        principal.latest_recorded_at = principal.latest_recorded_at.max(entry.recorded_at);
        if let Some(files) = entry.files {
            principal.files = principal.files.saturating_add(files);
        }
        if let Some(bytes) = entry.content_bytes {
            principal.content_bytes = principal.content_bytes.saturating_add(bytes);
        }
        if let Some(bytes) = entry.replica_bytes_estimate {
            principal.replica_bytes_estimate =
                principal.replica_bytes_estimate.saturating_add(bytes);
        }
        if entry
            .storage_quota
            .get("enforced")
            .or_else(|| entry.quota.get("enforced"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            principal.quota_enforced = principal.quota_enforced.saturating_add(1);
        }
    }

    let mut top_principals = principals
        .into_iter()
        .map(|(principal_did, totals)| {
            json!({
                "principal_did": principal_did,
                "active_objects": totals.active_objects,
                "files": totals.files,
                "content_bytes": totals.content_bytes,
                "replica_bytes_estimate": totals.replica_bytes_estimate,
                "quota_enforced": totals.quota_enforced,
                "latest_recorded_at": totals.latest_recorded_at,
            })
        })
        .collect::<Vec<_>>();
    top_principals.sort_by(|a, b| {
        b.get("content_bytes")
            .and_then(|value| value.as_u64())
            .unwrap_or(0)
            .cmp(
                &a.get("content_bytes")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
            )
            .then_with(|| {
                a.get("principal_did")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .cmp(
                        b.get("principal_did")
                            .and_then(|value| value.as_str())
                            .unwrap_or(""),
                    )
            })
    });
    let top_principals_truncated = top_principals.len() > AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT;
    top_principals.truncate(AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT);

    let active_objects = storage_ledger
        .get("active_objects")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let quota_enforced = storage_ledger
        .get("quota_enforced")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let pressure_level = if quota_exceeded_records > 0 {
        "quota_exceeded"
    } else if quota_enforced > 0 {
        "quota_enforced"
    } else if active_objects > 0 {
        "accounting_observed"
    } else {
        "idle"
    };

    let mut by_task_status: BTreeMap<String, u32> = BTreeMap::new();
    let mut due = 0_u32;
    let mut next_due_after: Option<u64> = None;
    let mut total_attempts = 0_u64;
    let mut tasks_with_failures = 0_u32;
    for task in tasks {
        *by_task_status.entry(task.status.clone()).or_insert(0) += 1;
        total_attempts = total_attempts.saturating_add(task.attempts as u64);
        if task.attempts > 0 && !matches!(task.status.as_str(), "healthy" | "local_only") {
            tasks_with_failures = tasks_with_failures.saturating_add(1);
        }
        if task.is_repair_candidate(true) {
            if task.is_due(now) {
                due = due.saturating_add(1);
            } else if task.next_check_after > 0 {
                next_due_after = Some(
                    next_due_after
                        .map(|current| current.min(task.next_check_after))
                        .unwrap_or(task.next_check_after),
                );
            }
        }
    }

    let mut recent_tasks = tasks.to_vec();
    recent_tasks.sort_by(|a, b| {
        b.checked_at
            .cmp(&a.checked_at)
            .then_with(|| a.cid.cmp(&b.cid))
    });
    let recent_fleet_history = recent_tasks
        .iter()
        .take(AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT)
        .map(|task| {
            json!({
                "cid": task.cid,
                "status": task.status,
                "policy": task.policy,
                "attempts": task.attempts,
                "checked_at": task.checked_at,
                "next_check_after": task.next_check_after,
                "due": task.is_due(now),
                "reason": task.reason.clone(),
            })
        })
        .collect::<Vec<_>>();

    let live_multi_peer_proofs = receipts
        .iter()
        .filter(|receipt| {
            receipt
                .payload
                .peer_selection
                .get("live_multi_peer_proof")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
        .count();

    json!({
        "schema": CONTENT_OPERATOR_DASHBOARD_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "authority": {
            "runtime_invocation_required": true,
            "app_visible": false,
            "raw_backend_access": false,
        },
        "storage_pressure": {
            "status": pressure_level,
            "tracked_objects": storage_ledger
                .get("tracked_objects")
                .cloned()
                .unwrap_or(Value::from(storage_entries.len() as u64)),
            "active_objects": active_objects,
            "tracked_principals": storage_ledger
                .get("tracked_principals")
                .cloned()
                .unwrap_or(Value::from(0)),
            "active_principals": storage_ledger
                .get("active_principals")
                .cloned()
                .unwrap_or(Value::from(0)),
            "content_bytes": storage_ledger
                .get("content_bytes")
                .cloned()
                .unwrap_or(Value::from(0)),
            "replica_bytes_estimate": storage_ledger
                .get("replica_bytes_estimate")
                .cloned()
                .unwrap_or(Value::from(0)),
            "quota_enforced": quota_enforced,
            "quota_exceeded_records": quota_exceeded_records,
            "quota_ledger_policy": federated_quota_ledger_policy_status_json(
                receipts,
                storage_ledger,
                integrations.federated_quota_ledger_exchange,
            ),
            "top_principal_limit": AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT,
            "top_principals_truncated": top_principals_truncated,
            "top_principals_by_content_bytes": top_principals,
            "market_policy": storage_ledger
                .get("market_policy")
                .cloned()
                .unwrap_or_else(default_content_storage_market_json),
            "settlement_policy": storage_ledger
                .get("market_policy")
                .map(storage_settlement_policy_from_market_json)
                .unwrap_or_else(default_storage_settlement_policy_json),
            "market_admission_policy": storage_ledger
                .get("market_policy")
                .map(storage_market_admission_policy_from_market_json)
                .unwrap_or_else(default_storage_market_admission_policy_json),
        },
        "fleet_history": {
            "tracked_tasks": tasks.len(),
            "by_status": by_task_status,
            "due": due,
            "next_due_after": next_due_after,
            "total_attempts": total_attempts,
            "tasks_with_failures": tasks_with_failures,
            "recent_limit": AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT,
            "recent": recent_fleet_history,
            "external_repair_fleet_policy": external_repair_fleet_policy_json(
                tasks,
                now,
                integrations.external_repair_fleet,
            ),
        },
        "proof_summary": {
            "signed_receipts": receipts.len(),
            "live_multi_peer_proofs": live_multi_peer_proofs,
            "peer_attestation_exchange_policy": peer_attestation_exchange_policy_status_json(
                receipts,
            ),
        },
        "federated_operator_alerting_policy": federated_operator_alerting_policy_json(
            receipts,
            tasks,
            storage_entries,
            storage_ledger,
            now,
            integrations.operator_alert_sink,
            integrations.federated_operator_alert_exchange,
        ),
        "production_federation": {
            "configured": integrations.federated_operator_alert_exchange.is_some(),
            "external_repair_fleets": false,
            "federated_storage_pressure": false,
            "operator_alerting": if integrations.federated_operator_alert_exchange.is_some() {
                "configured_federated_alert_exchange"
            } else if integrations.operator_alert_sink.is_some() {
                "provider_local_webhook"
            } else {
                "provider_status_only"
            },
            "reason": if integrations.federated_operator_alert_exchange.is_some() {
                "this branch can exchange provider alert receipts with one configured operator-owned federated endpoint; production dashboards still need subscribed provider fleets, peer-health feeds, and operator UI"
            } else if integrations.operator_alert_sink.is_some() {
                "this branch can deliver provider-local alert receipts to one configured operator sink; production dashboards still need federated ledgers, repair fleets, and alert exchange across independent providers"
            } else {
                "this branch exposes provider-local storage pressure and fleet history; production dashboards need federated ledgers, repair fleets, and alerting across independent providers"
            },
        },
    })
}

fn operator_alert_payload(dashboard: &Value, emitted_at: u64) -> Value {
    let alerting_policy = dashboard
        .get("federated_operator_alerting_policy")
        .cloned()
        .unwrap_or(Value::Null);
    let local_signals = alerting_policy
        .get("local_signals")
        .cloned()
        .unwrap_or(Value::Null);
    let federation = alerting_policy
        .get("federation")
        .cloned()
        .unwrap_or(Value::Null);
    let operator_dashboard = dashboard.get("operator_dashboard").unwrap_or(&Value::Null);
    json!({
        "schema": CONTENT_OPERATOR_ALERT_SCHEMA,
        "provider": "content-provider",
        "scope": "content-availability",
        "emitted_at": emitted_at,
        "dashboard_schema": dashboard.get("schema").cloned().unwrap_or(Value::Null),
        "policy": alerting_policy
            .get("policy")
            .cloned()
            .unwrap_or(Value::Null),
        "status": alerting_policy
            .get("status")
            .cloned()
            .unwrap_or(Value::Null),
        "local_signals": local_signals,
        "storage_pressure": operator_dashboard
            .get("storage_pressure")
            .cloned()
            .unwrap_or(Value::Null),
        "repair_pressure": operator_dashboard
            .get("fleet_history")
            .cloned()
            .unwrap_or(Value::Null),
        "authority": {
            "runtime_invocation_required": true,
            "provider_owned_sink": true,
            "credential_exposed": false,
            "raw_backend_access": false,
        },
        "production_federation": {
            "configured": federation
                .get("configured")
                .cloned()
                .unwrap_or(Value::Bool(false)),
            "cross_provider_dashboard": federation
                .get("cross_provider_dashboard")
                .cloned()
                .unwrap_or(Value::Bool(false)),
            "fleet_alert_exchange": federation
                .get("fleet_alert_exchange")
                .cloned()
                .unwrap_or(Value::Bool(false)),
            "federated_alert_receipts": federation
                .get("federated_alert_receipts")
                .cloned()
                .unwrap_or(Value::Bool(false)),
            "reason": federation
                .get("reason")
                .cloned()
                .unwrap_or_else(|| Value::String("provider alert payload follows the current provider-owned alerting policy".to_string())),
        },
    })
}

fn federated_operator_alerting_policy_json(
    receipts: &[SignedAvailabilityReceipt],
    tasks: &[ContentRepairTask],
    storage_entries: &[ContentStorageAccountingEntry],
    storage_ledger: &Value,
    now: u64,
    operator_alert_sink: Option<&ContentOperatorAlertSink>,
    federated_operator_alert_exchange: Option<&ContentFederatedOperatorAlertExchangeClient>,
) -> Value {
    let active_objects = storage_ledger
        .get("active_objects")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let content_bytes = storage_ledger
        .get("content_bytes")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let replica_bytes_estimate = storage_ledger
        .get("replica_bytes_estimate")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let quota_enforced = storage_ledger
        .get("quota_enforced")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let quota_exceeded_records = storage_entries
        .iter()
        .filter(|entry| {
            entry
                .storage_quota
                .get("status")
                .or_else(|| entry.quota.get("status"))
                .and_then(|value| value.as_str())
                == Some("quota_exceeded")
        })
        .count();
    let storage_pressure_status = if quota_exceeded_records > 0 {
        "quota_exceeded"
    } else if quota_enforced > 0 {
        "quota_enforced"
    } else if active_objects > 0 {
        "accounting_observed"
    } else {
        "idle"
    };

    let queued_tasks = tasks.iter().filter(|task| task.status == "queued").count();
    let due_tasks = tasks
        .iter()
        .filter(|task| task.is_repair_candidate(true) && task.is_due(now))
        .count();
    let failed_tasks = tasks
        .iter()
        .filter(|task| {
            task.attempts > 0 && !matches!(task.status.as_str(), "healthy" | "local_only")
        })
        .count();
    let repair_pressure_status = if due_tasks > 0 {
        "repair_due"
    } else if queued_tasks > 0 {
        "repair_queued"
    } else if failed_tasks > 0 {
        "repair_failures_observed"
    } else {
        "idle"
    };

    let live_multi_peer_proofs = receipts
        .iter()
        .filter(|receipt| {
            receipt
                .payload
                .peer_selection
                .get("live_multi_peer_proof")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
        .count();
    let mut remote_replicas = 0_u32;
    let mut verified_remote_receipts = 0_u32;
    for receipt in receipts {
        for replica in receipt
            .payload
            .peer_selection
            .get("replicas")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if replica.get("role").and_then(|value| value.as_str()) == Some("remote") {
                remote_replicas = remote_replicas.saturating_add(1);
            }
            if replica
                .get("remote_receipt")
                .and_then(|remote| remote.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                verified_remote_receipts = verified_remote_receipts.saturating_add(1);
            }
        }
    }

    json!({
        "schema": CONTENT_FEDERATED_OPERATOR_ALERTING_POLICY_SCHEMA,
        "policy": if federated_operator_alert_exchange.is_some() {
            "provider_local_dashboard_with_federated_alert_exchange"
        } else if operator_alert_sink.is_some() {
            "provider_local_dashboard_with_operator_alert_sink"
        } else {
            "provider_local_dashboard_no_federated_alerting"
        },
        "scope": "content-availability",
        "status": if federated_operator_alert_exchange.is_some() {
            "federated_alert_exchange_configured"
        } else if operator_alert_sink.is_some() {
            "provider_local_alert_sink_configured"
        } else if receipts.is_empty() && tasks.is_empty() && active_objects == 0 {
            "idle_no_federated_dashboard"
        } else {
            "provider_local_dashboard_only"
        },
        "authority": {
            "provider": "content-provider",
            "runtime_invocation_required": true,
            "app_visible": false,
            "raw_backend_access": false,
        },
        "local_dashboard": {
            "available": true,
            "schema": CONTENT_OPERATOR_DASHBOARD_SCHEMA,
            "provider_wide_status": true,
            "per_cid_status": true,
            "repair_worker_runs": true,
            "status_json_only": true,
            "operator_ui": "not_configured",
        },
        "operator_alert_sink": operator_alert_sink
            .map(ContentOperatorAlertSink::redacted_status_json)
            .unwrap_or_else(|| {
                json!({
                    "configured": false,
                    "delivery": "not_configured",
                    "authorization_configured": false,
                })
            }),
        "federated_alert_exchange": federated_operator_alert_exchange
            .map(ContentFederatedOperatorAlertExchangeClient::redacted_status_json)
            .unwrap_or_else(|| {
                json!({
                    "configured": false,
                    "delivery": "not_configured",
                    "authorization_configured": false,
                })
            }),
        "local_signals": {
            "signed_receipts": receipts.len(),
            "tracked_tasks": tasks.len(),
            "queued_tasks": queued_tasks,
            "due_tasks": due_tasks,
            "failed_tasks": failed_tasks,
            "storage_pressure_status": storage_pressure_status,
            "active_objects": active_objects,
            "content_bytes": content_bytes,
            "replica_bytes_estimate": replica_bytes_estimate,
            "quota_enforced": quota_enforced,
            "quota_exceeded_records": quota_exceeded_records,
            "live_multi_peer_proofs": live_multi_peer_proofs,
            "remote_replicas": remote_replicas,
            "verified_remote_receipts": verified_remote_receipts,
            "repair_pressure_status": repair_pressure_status,
        },
        "federation": {
            "configured": federated_operator_alert_exchange.is_some(),
            "cross_provider_dashboard": false,
            "alert_delivery": operator_alert_sink.is_some() || federated_operator_alert_exchange.is_some(),
            "fleet_alert_exchange": federated_operator_alert_exchange.is_some(),
            "federated_alert_receipts": federated_operator_alert_exchange.is_some(),
            "peer_health_subscription": false,
            "storage_pressure_alerts": if federated_operator_alert_exchange.is_some() {
                "federated_alert_exchange"
            } else if operator_alert_sink.is_some() {
                "provider_local_webhook"
            } else {
                "provider_status_only"
            },
            "repair_pressure_alerts": if federated_operator_alert_exchange.is_some() {
                "federated_alert_exchange"
            } else if operator_alert_sink.is_some() {
                "provider_local_webhook"
            } else {
                "provider_status_only"
            },
            "reason": if federated_operator_alert_exchange.is_some() {
                "provider alert receipts can be delivered to one configured operator-owned federated exchange; cross-provider dashboards, peer-health subscriptions, operator UI, and fleet-wide SLA policy remain production-network work"
            } else if operator_alert_sink.is_some() {
                "provider-local alert delivery can post signed local signals to one configured operator sink; configure a federated operator alert exchange for exchange receipts, while production dashboards, peer-health subscriptions, operator UI, and fleet-wide SLA policy remain production-network work"
            } else {
                "this branch exposes provider-local status JSON only; federated dashboards and alert delivery require independent provider networks, signed operator subscriptions, and cross-provider alert receipts"
            },
        },
    })
}

fn repair_fleet_status_json(tasks: &[ContentRepairTask], now: u64) -> Value {
    let mut by_status: BTreeMap<String, u32> = BTreeMap::new();
    let mut queued = 0_u32;
    let mut due = 0_u32;
    let mut healthy = 0_u32;
    let mut eligible_now = 0_u32;
    let mut next_due_after: Option<u64> = None;

    for task in tasks {
        *by_status.entry(task.status.clone()).or_insert(0) += 1;
        if task.status == "queued" {
            queued = queued.saturating_add(1);
        }
        if task.status == "healthy" {
            healthy = healthy.saturating_add(1);
        }
        if task.is_repair_candidate(false) && task.is_due(now) {
            eligible_now = eligible_now.saturating_add(1);
        }
        if matches!(task.status.as_str(), "queued" | "healthy") {
            if task.is_due(now) {
                due = due.saturating_add(1);
            } else if task.next_check_after > 0 {
                next_due_after = Some(
                    next_due_after
                        .map(|current| current.min(task.next_check_after))
                        .unwrap_or(task.next_check_after),
                );
            }
        }
    }

    json!({
        "schema": REPAIR_FLEET_SCHEMA,
        "policy": "single_runtime_provider_repair_fleet",
        "scope": "content-availability",
        "status": if due > 0 {
            "work_due"
        } else if queued > 0 {
            "scheduled"
        } else if healthy > 0 {
            "healthy_monitoring"
        } else {
            "idle"
        },
        "coordinator": {
            "provider": "content-provider",
            "authority": "provider-owned-repair-ledger",
            "app_visible": false,
        },
        "workers": [{
            "provider": "content-provider",
            "role": "local_repair_worker",
            "manual_trigger": "elastos content repair-worker",
            "scheduler_env": "ELASTOS_CONTENT_REPAIR_SCHEDULER",
            "enabled_by_default": false,
            "runtime_invocation_required": true,
        }],
        "scheduling": {
            "source": "content_repair_task_ledger",
            "retry_delay_secs": REPAIR_RETRY_DELAY_SECS,
            "healthy_check_delay_secs": REPAIR_HEALTH_CHECK_DELAY_SECS,
            "default_limit": REPAIR_WORKER_DEFAULT_LIMIT,
            "max_limit": REPAIR_WORKER_MAX_LIMIT,
            "default_max_attempts": REPAIR_WORKER_DEFAULT_MAX_ATTEMPTS,
            "default_failure_budget": REPAIR_WORKER_DEFAULT_FAILURE_BUDGET,
            "eligible_now": eligible_now,
            "next_due_after": next_due_after,
        },
        "task_pressure": {
            "tracked": tasks.len(),
            "queued": queued,
            "due": due,
            "healthy": healthy,
            "by_status": by_status,
        },
        "production_federation": {
            "configured": false,
            "external_workers": false,
            "fleet_settlement": "not_configured",
            "storage_market_admission": "not_configured",
            "reason": "this branch exposes the provider-owned repair-fleet policy; external repair fleets and storage markets remain production-network work",
        },
    })
}

fn external_repair_fleet_policy_json(
    tasks: &[ContentRepairTask],
    now: u64,
    external_repair_fleet: Option<&ContentExternalRepairFleetClient>,
) -> Value {
    let queued = tasks.iter().filter(|task| task.status == "queued").count();
    let due = tasks
        .iter()
        .filter(|task| task.is_repair_candidate(false) && task.is_due(now))
        .count();
    let healthy = tasks.iter().filter(|task| task.status == "healthy").count();
    json!({
        "schema": EXTERNAL_REPAIR_FLEET_POLICY_SCHEMA,
        "policy": if external_repair_fleet.is_some() {
            "provider_owned_repair_with_external_dispatch"
        } else {
            "single_runtime_provider_owned_repair"
        },
        "scope": "content-availability",
        "status": if external_repair_fleet.is_some() {
            "external_repair_fleet_dispatch_configured"
        } else {
            "external_repair_fleet_not_configured"
        },
        "local_runtime": {
            "coordinator": "content-provider",
            "worker": "content-provider",
            "task_ledger": REPAIR_TASK_SCHEMA,
            "manual_trigger": "elastos content repair-worker",
            "scheduler_env": "ELASTOS_CONTENT_REPAIR_SCHEDULER",
            "tracked_tasks": tasks.len(),
            "queued": queued,
            "due": due,
            "healthy": healthy,
        },
        "external_fleet": {
            "configured": external_repair_fleet.is_some(),
            "coordinator": if external_repair_fleet.is_some() {
                "operator_configured_dispatch_endpoint_quorum"
            } else {
                "not_configured"
            },
            "dispatch": external_repair_fleet
                .map(ContentExternalRepairFleetClient::redacted_status_json)
                .unwrap_or_else(|| {
                    json!({
                        "configured": false,
                        "delivery": "not_configured",
                        "authorization_configured": false,
                    })
                }),
            "workers": external_repair_fleet
                .map(ContentExternalRepairFleetClient::endpoint_count)
                .unwrap_or(0),
            "volunteer_workers": false,
            "supernode_workers": external_repair_fleet.is_some(),
            "cross_provider_repair_queue": external_repair_fleet.is_some(),
            "fleet_settlement": "not_configured",
            "storage_market_admission": "not_configured",
        },
        "federation": {
            "configured": external_repair_fleet.is_some(),
            "repair_task_exchange": external_repair_fleet.is_some(),
            "worker_attestation_receipts": false,
            "cross_provider_repair_sla": false,
            "reason": if external_repair_fleet.is_some() {
                "content repair-worker can dispatch due tasks to a configured external repair-fleet endpoint quorum; worker attestations, settlement, and cross-provider repair SLAs remain production work"
            } else {
                "this branch exposes a provider-owned local repair worker and scheduler posture only; production external repair fleets require cross-provider task exchange, worker attestations, and SLA policy"
            },
        },
    })
}

struct ExternalRepairFleetRunSummary {
    checked: u32,
    repaired: u32,
    failed: u32,
    skipped: u32,
    exhausted_attempts_skipped: u32,
    throttled: bool,
    external_dispatches: u32,
    external_dispatch_accepted: u32,
    external_dispatch_failed: u32,
}

fn external_repair_fleet_run_policy_json(
    run: ExternalRepairFleetRunSummary,
    external_repair_fleet: Option<&ContentExternalRepairFleetClient>,
) -> Value {
    json!({
        "schema": EXTERNAL_REPAIR_FLEET_POLICY_SCHEMA,
        "policy": if external_repair_fleet.is_some() {
            "provider_owned_repair_with_external_dispatch"
        } else {
            "single_runtime_provider_owned_repair"
        },
        "scope": "content-availability",
        "status": if external_repair_fleet.is_some() {
            "external_repair_fleet_dispatch_configured"
        } else {
            "external_repair_fleet_not_configured"
        },
        "run": {
            "worker": "content-provider",
            "checked": run.checked,
            "repaired": run.repaired,
            "failed": run.failed,
            "skipped": run.skipped,
            "exhausted_attempts_skipped": run.exhausted_attempts_skipped,
            "throttled": run.throttled,
            "external_dispatches": run.external_dispatches,
            "external_dispatch_accepted": run.external_dispatch_accepted,
            "external_dispatch_failed": run.external_dispatch_failed,
        },
        "external_fleet": {
            "configured": external_repair_fleet.is_some(),
            "coordinator": if external_repair_fleet.is_some() {
                "operator_configured_dispatch_endpoint_quorum"
            } else {
                "not_configured"
            },
            "dispatch": external_repair_fleet
                .map(ContentExternalRepairFleetClient::redacted_status_json)
                .unwrap_or_else(|| {
                    json!({
                        "configured": false,
                        "delivery": "not_configured",
                        "authorization_configured": false,
                    })
                }),
            "workers": external_repair_fleet
                .map(ContentExternalRepairFleetClient::endpoint_count)
                .unwrap_or(0),
            "cross_provider_repair_queue": external_repair_fleet.is_some(),
            "fleet_settlement": "not_configured",
        },
    })
}

fn repair_fleet_run_json(
    checked: u32,
    repaired: u32,
    failed: u32,
    skipped: u32,
    exhausted_attempts_skipped: u32,
    throttled: bool,
) -> Value {
    json!({
        "schema": REPAIR_FLEET_SCHEMA,
        "policy": "single_runtime_provider_repair_fleet",
        "scope": "content-availability",
        "run_mode": "provider_owned_worker",
        "coordinator": "content-provider",
        "worker": "content-provider",
        "checked": checked,
        "repaired": repaired,
        "failed": failed,
        "skipped": skipped,
        "exhausted_attempts_skipped": exhausted_attempts_skipped,
        "throttled": throttled,
        "production_federation": {
            "configured": false,
            "external_workers": false,
            "fleet_settlement": "not_configured",
        },
    })
}

fn local_peer_selection_json() -> Value {
    json!({
        "mode": "single_local",
        "live_multi_peer_proof": false,
    })
}

fn federated_quota_ledger_policy_json(
    mode: &str,
    quota_status: &str,
    local_principal_ledger: bool,
    remote_admission_preflight: bool,
    enforced: bool,
) -> Value {
    json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA,
        "policy": "local_principal_ledger_plus_remote_admission_preflight",
        "scope": "content-availability",
        "status": "federated_quota_ledger_not_configured",
        "quota": {
            "mode": mode,
            "status": quota_status,
            "enforced": enforced,
        },
        "local": {
            "principal_storage_ledger": local_principal_ledger,
            "ledger_schema": CONTENT_STORAGE_ACCOUNTING_LEDGER_SCHEMA,
            "storage_quota_schema": CONTENT_STORAGE_QUOTA_SCHEMA,
        },
        "remote": {
            "admission_preflight": remote_admission_preflight,
            "signed_admission_receipts": remote_admission_preflight,
            "admission_schema": CONTENT_ADMISSION_SCHEMA,
            "admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
        },
        "federation": {
            "configured": false,
            "cross_provider_quota_ledger": false,
            "storage_admission_network": false,
            "signed_admission_receipt_exchange": remote_admission_preflight,
            "quota_receipt_exchange": false,
            "production_quota_receipt_exchange": false,
            "reason": if remote_admission_preflight {
                "signed remote content/admission receipt exchange exists for the proof path; federated quota ledgers and production storage-admission networks remain unconfigured"
            } else {
                "local per-principal quota exists, but remote signed admission and federated quota ledgers are not configured for this path"
            },
        },
    })
}

fn default_federated_quota_ledger_policy_json() -> Value {
    federated_quota_ledger_policy_json("not_reported", "not_reported", false, false, false)
}

fn federated_quota_ledger_policy_from_quota_json(
    quota: &Value,
    remote_admission_preflight: bool,
) -> Value {
    if let Some(policy) = quota.get("federated_quota_ledger_policy") {
        return policy.clone();
    }
    let quota_policy = quota
        .get("policy")
        .and_then(|value| value.as_str())
        .unwrap_or("not_reported");
    let local_principal_ledger = matches!(
        quota_policy,
        "principal_storage_quota" | "carrier_provider_quota"
    );
    federated_quota_ledger_policy_json(
        quota_policy,
        quota
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported"),
        local_principal_ledger,
        remote_admission_preflight,
        quota
            .get("enforced")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    )
}

fn federated_quota_ledger_policy_from_exchange(quota: &Value, exchange: &Value) -> Value {
    let quota_policy = quota
        .get("policy")
        .and_then(|value| value.as_str())
        .unwrap_or("not_reported");
    let quota_status = quota
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("not_reported");
    let local_principal_ledger = matches!(
        quota_policy,
        "principal_storage_quota" | "carrier_provider_quota"
    );
    let enforced = quota
        .get("enforced")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let exchange_accepted = exchange
        .get("accepted")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let exchange_status = exchange
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("not_reported");
    json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA,
        "policy": "configured_federated_quota_ledger_exchange",
        "scope": "content-availability",
        "status": if exchange_accepted {
            "federated_quota_ledger_accepted"
        } else {
            "federated_quota_ledger_rejected"
        },
        "quota": {
            "mode": quota_policy,
            "status": quota_status,
            "enforced": enforced,
        },
        "local": {
            "principal_storage_ledger": local_principal_ledger,
            "ledger_schema": CONTENT_STORAGE_ACCOUNTING_LEDGER_SCHEMA,
            "storage_quota_schema": CONTENT_STORAGE_QUOTA_SCHEMA,
        },
        "remote": {
            "admission_preflight": true,
            "signed_admission_receipts": true,
            "admission_schema": CONTENT_ADMISSION_SCHEMA,
            "admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
        },
        "federation": {
            "configured": true,
            "cross_provider_quota_ledger": true,
            "storage_admission_network": false,
            "signed_admission_receipt_exchange": true,
            "quota_receipt_exchange": true,
            "production_quota_receipt_exchange": false,
            "exchange_schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA,
            "exchange_receipt_schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA,
            "exchange_status": exchange_status,
            "signed_exchange_receipt_verified": exchange
                .get("signed_receipt")
                .and_then(|value| value.get("verified"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            "reason": if exchange_accepted {
                "configured federated quota-ledger exchange accepted this admission preflight"
            } else {
                "configured federated quota-ledger exchange rejected or could not verify this admission preflight"
            },
        },
    })
}

fn federated_quota_ledger_policy_status_json(
    receipts: &[SignedAvailabilityReceipt],
    storage_ledger: &Value,
    federated_quota_ledger_exchange: Option<&ContentFederatedQuotaLedgerExchangeClient>,
) -> Value {
    let mut by_status: BTreeMap<String, u32> = BTreeMap::new();
    let mut local_quota_receipts = 0_u32;
    let mut remote_admission_receipts = 0_u32;
    let mut federated_policy_receipts = 0_u32;

    for receipt in receipts {
        let mut receipt_remote_admission = false;
        for replica in receipt
            .payload
            .peer_selection
            .get("replicas")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
        {
            if replica.get("admission").is_some() {
                receipt_remote_admission = true;
                remote_admission_receipts = remote_admission_receipts.saturating_add(1);
            }
        }
        let policy = federated_quota_ledger_policy_from_quota_json(
            &receipt.payload.quota,
            receipt_remote_admission,
        );
        let status = policy
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported")
            .to_string();
        *by_status.entry(status).or_insert(0) += 1;
        if policy
            .get("local")
            .and_then(|value| value.get("principal_storage_ledger"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            local_quota_receipts = local_quota_receipts.saturating_add(1);
        }
        if policy
            .get("federation")
            .and_then(|value| value.get("configured"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            federated_policy_receipts = federated_policy_receipts.saturating_add(1);
        }
    }

    json!({
        "schema": CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA,
        "policy": "provider_receipt_quota_ledger_status",
        "scope": "content-availability",
        "status": if federated_policy_receipts > 0 || federated_quota_ledger_exchange.is_some() {
            "federated_quota_ledger_observed"
        } else {
            "federated_quota_ledger_not_configured"
        },
        "receipt_count": receipts.len(),
        "by_status": by_status,
        "local": {
            "principal_storage_ledger": true,
            "ledger_schema": CONTENT_STORAGE_ACCOUNTING_LEDGER_SCHEMA,
            "tracked_objects": storage_ledger
                .get("tracked_objects")
                .cloned()
                .unwrap_or(Value::from(0)),
            "quota_enforced": storage_ledger
                .get("quota_enforced")
                .cloned()
                .unwrap_or(Value::from(0)),
            "quota_receipts": local_quota_receipts,
        },
        "remote": {
            "admission_preflight": remote_admission_receipts > 0,
            "admission_receipts": remote_admission_receipts,
            "signed_admission_receipts": remote_admission_receipts,
            "admission_schema": CONTENT_ADMISSION_SCHEMA,
            "admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
        },
        "federation": {
            "configured": federated_policy_receipts > 0 || federated_quota_ledger_exchange.is_some(),
            "cross_provider_quota_ledger": federated_policy_receipts > 0 || federated_quota_ledger_exchange.is_some(),
            "storage_admission_network": false,
            "signed_admission_receipt_exchange": remote_admission_receipts > 0,
            "quota_receipt_exchange": federated_policy_receipts > 0 || federated_quota_ledger_exchange.is_some(),
            "production_quota_receipt_exchange": false,
            "exchange_client": federated_quota_ledger_exchange
                .map(ContentFederatedQuotaLedgerExchangeClient::redacted_status_json)
                .unwrap_or_else(|| {
                    json!({
                        "configured": false,
                        "delivery": "not_configured",
                        "authorization_configured": false,
                    })
                }),
            "reason": if federated_policy_receipts > 0 {
                "one or more receipts reported federated quota-ledger policy"
            } else if federated_quota_ledger_exchange.is_some() {
                "configured federated quota-ledger exchange is enforced by content/admission before remote bytes or DAG repair data move"
            } else if remote_admission_receipts > 0 {
                "signed remote content/admission receipts were observed, but no federated quota ledger or production storage-admission network is configured"
            } else {
                "no federated quota ledger or cross-provider quota-receipt exchange is configured"
            },
        },
    })
}

fn local_quota_json() -> Value {
    json!({
        "policy": "not_enforced",
        "scope": "local_content_backend",
        "federated_quota_ledger_policy": default_federated_quota_ledger_policy_json(),
    })
}

fn storage_market_admission_policy_json(
    mode: &str,
    market_status: &str,
    quota_enforced: bool,
    live_multi_peer_proof: bool,
    remote_admission_preflight: bool,
) -> Value {
    json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA,
        "policy": "proof_path_admission_no_production_market",
        "scope": "content-availability",
        "status": if remote_admission_preflight {
            "remote_admission_preflight_no_market_admission"
        } else if quota_enforced {
            "local_quota_admission_no_market_admission"
        } else {
            "production_storage_market_admission_not_configured"
        },
        "market": {
            "mode": mode,
            "status": market_status,
            "quota_enforced": quota_enforced,
            "live_multi_peer_proof": live_multi_peer_proof,
        },
        "current_admission": {
            "local_principal_quota_ledger": quota_enforced,
            "remote_content_admission_preflight": remote_admission_preflight,
            "signed_admission_receipts": remote_admission_preflight,
            "content_admission_schema": CONTENT_ADMISSION_SCHEMA,
            "content_admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
            "provider_invocation_required": true,
            "signed_availability_receipts": true,
        },
        "production_market": {
            "configured": false,
            "provider_admission_network": false,
            "provider_offer_receipts": false,
            "price_discovery": false,
            "sla_admission": false,
            "abuse_economic_controls": false,
            "reason": "this branch admits storage through local quota and signed bounded remote content/admission receipts; production storage-market admission needs provider offers, pricing, SLA, and trust policy receipts",
        },
    })
}

fn default_storage_market_admission_policy_json() -> Value {
    storage_market_admission_policy_json("not_reported", "not_reported", false, false, false)
}

fn storage_market_admission_policy_from_market_json(storage_market: &Value) -> Value {
    if let Some(policy) = storage_market.get("admission_policy") {
        return policy.clone();
    }
    storage_market_admission_policy_json(
        storage_market
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported"),
        storage_market
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported"),
        storage_market
            .get("quota_enforced")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        storage_market
            .get("live_multi_peer_proof")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        storage_market
            .get("remote_admission_preflight")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    )
}

fn storage_market_admission_policy_status_json(
    receipts: &[SignedAvailabilityReceipt],
    storage_ledger: &Value,
    storage_market_admission: Option<&ContentStorageMarketAdmissionClient>,
) -> Value {
    let mut by_status: BTreeMap<String, u32> = BTreeMap::new();
    let mut policy_receipts = 0_u32;
    let mut production_configured = 0_u32;
    let mut local_quota_admission = 0_u32;
    let mut remote_admission_preflight = 0_u32;

    for receipt in receipts {
        let policy =
            storage_market_admission_policy_from_market_json(&receipt.payload.storage_market);
        policy_receipts = policy_receipts.saturating_add(1);
        let status = policy
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported")
            .to_string();
        *by_status.entry(status).or_insert(0) += 1;
        if policy
            .get("production_market")
            .and_then(|value| value.get("configured"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            production_configured = production_configured.saturating_add(1);
        }
        if policy
            .get("current_admission")
            .and_then(|value| value.get("local_principal_quota_ledger"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            local_quota_admission = local_quota_admission.saturating_add(1);
        }
        if policy
            .get("current_admission")
            .and_then(|value| value.get("remote_content_admission_preflight"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            remote_admission_preflight = remote_admission_preflight.saturating_add(1);
        }
    }

    let ledger_policy = storage_ledger
        .get("market_policy")
        .map(storage_market_admission_policy_from_market_json)
        .unwrap_or_else(default_storage_market_admission_policy_json);
    json!({
        "schema": CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA,
        "policy": "provider_receipt_storage_market_admission_status",
        "scope": "content-availability",
        "status": if production_configured > 0 || storage_market_admission.is_some() {
            "production_storage_market_admission_observed"
        } else if remote_admission_preflight > 0 {
            "remote_admission_preflight_no_market_admission"
        } else if local_quota_admission > 0 {
            "local_quota_admission_no_market_admission"
        } else {
            "production_storage_market_admission_not_configured"
        },
        "receipt_count": receipts.len(),
        "policy_receipts": policy_receipts,
        "by_status": by_status,
        "ledger_policy": ledger_policy,
        "current_admission": {
            "local_quota_receipts": local_quota_admission,
            "remote_admission_preflight_receipts": remote_admission_preflight,
            "signed_admission_receipts": remote_admission_preflight,
            "content_admission_schema": CONTENT_ADMISSION_SCHEMA,
            "content_admission_receipt_domain": CONTENT_ADMISSION_DOMAIN,
            "provider_invocation_required": true,
        },
        "external_admission_client": storage_market_admission
            .map(ContentStorageMarketAdmissionClient::redacted_status_json)
            .unwrap_or_else(|| {
                json!({
                    "configured": false,
                    "delivery": "not_configured",
                    "authorization_configured": false,
                })
            }),
        "production_market": {
            "configured": production_configured > 0 || storage_market_admission.is_some(),
            "admission_policy_receipts": production_configured,
            "provider_admission_network": production_configured > 0 || storage_market_admission.is_some(),
            "provider_offer_receipts": storage_market_admission.is_some(),
            "price_discovery": false,
            "sla_admission": false,
            "reason": if production_configured > 0 {
                "one or more receipts reported production storage-market admission"
            } else if storage_market_admission.is_some() {
                "external storage-market admission is configured and enforced by content/admission before remote bytes or DAG repair data move"
            } else {
                "no production storage-market admission, offer, pricing, or SLA policy is configured"
            },
        },
    })
}

fn storage_settlement_policy_json(mode: &str, market_status: &str, quota_enforced: bool) -> Value {
    json!({
        "schema": CONTENT_STORAGE_SETTLEMENT_POLICY_SCHEMA,
        "policy": "no_settlement_receipt_policy",
        "scope": "content-availability",
        "status": "settlement_not_configured",
        "market": {
            "mode": mode,
            "status": market_status,
            "quota_enforced": quota_enforced,
        },
        "authority": {
            "provider": "content-provider",
            "runtime_invocation_required": true,
            "app_visible": false,
        },
        "settlement": {
            "pricing": "not_configured",
            "escrow": "not_configured",
            "payment_settlement": "not_configured",
            "sla_enforcement": "not_configured",
        },
        "production_federation": {
            "configured": false,
            "storage_market_admission": false,
            "cross_provider_escrow": false,
            "settlement_receipts": false,
            "reason": "this branch records availability/accounting/quota posture only; pricing, escrow, settlement, and SLA policy require production storage-market providers",
        },
    })
}

fn default_storage_settlement_policy_json() -> Value {
    storage_settlement_policy_json("not_reported", "not_reported", false)
}

fn storage_settlement_policy_from_market_json(storage_market: &Value) -> Value {
    if let Some(policy) = storage_market.get("settlement_policy") {
        return policy.clone();
    }
    storage_settlement_policy_json(
        storage_market
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported"),
        storage_market
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported"),
        storage_market
            .get("quota_enforced")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    )
}

fn storage_settlement_policy_status_json(
    receipts: &[SignedAvailabilityReceipt],
    storage_ledger: &Value,
) -> Value {
    let mut by_status: BTreeMap<String, u32> = BTreeMap::new();
    let mut settlement_configured = 0_u32;
    for receipt in receipts {
        let policy = storage_settlement_policy_from_market_json(&receipt.payload.storage_market);
        let status = policy
            .get("status")
            .and_then(|value| value.as_str())
            .unwrap_or("not_reported")
            .to_string();
        *by_status.entry(status).or_insert(0) += 1;
        if policy
            .get("production_federation")
            .and_then(|value| value.get("configured"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            settlement_configured = settlement_configured.saturating_add(1);
        }
    }
    let ledger_policy = storage_ledger
        .get("market_policy")
        .map(storage_settlement_policy_from_market_json)
        .unwrap_or_else(default_storage_settlement_policy_json);
    json!({
        "schema": CONTENT_STORAGE_SETTLEMENT_POLICY_SCHEMA,
        "policy": "provider_receipt_settlement_status",
        "scope": "content-availability",
        "status": if settlement_configured > 0 {
            "settlement_policy_observed"
        } else {
            "settlement_not_configured"
        },
        "receipt_count": receipts.len(),
        "by_status": by_status,
        "ledger_policy": ledger_policy,
        "production_federation": {
            "configured": settlement_configured > 0,
            "settlement_policy_receipts": settlement_configured,
            "storage_market_admission": false,
            "cross_provider_escrow": false,
            "reason": if settlement_configured > 0 {
                "one or more receipts reported settlement policy"
            } else {
                "no production storage-market settlement, escrow, or pricing policy is configured"
            },
        },
    })
}

fn local_storage_market_json() -> Value {
    json!({
        "schema": "elastos.content.storage-market/v1",
        "mode": "local_content_backend",
        "status": "local_no_market_settlement",
        "settlement": "not_configured",
        "escrow": "not_configured",
        "quota_enforced": false,
        "admission_policy": storage_market_admission_policy_json(
            "local_content_backend",
            "local_no_market_settlement",
            false,
            false,
            false,
        ),
        "settlement_policy": storage_settlement_policy_json(
            "local_content_backend",
            "local_no_market_settlement",
            false,
        ),
    })
}

fn default_content_storage_market_json() -> Value {
    json!({
        "schema": "elastos.content.storage-market/v1",
        "mode": "not_reported",
        "status": "not_reported",
        "settlement": "not_configured",
        "escrow": "not_configured",
        "quota_enforced": false,
        "admission_policy": default_storage_market_admission_policy_json(),
        "settlement_policy": default_storage_settlement_policy_json(),
    })
}

fn is_default_content_storage_market_json(value: &Value) -> bool {
    value == &default_content_storage_market_json()
}

fn local_repair_graph_json() -> Value {
    json!({
        "schema": "elastos.content.repair-graph/v1",
        "policy": "content_provider_local_backend",
        "requested_kind": "auto",
        "status": "local_backend_only",
        "supported_import_fallbacks": ["object_manifest", "exact_bytes"],
        "refuses_exact_fallback_for_arbitrary_dag": true,
    })
}

fn default_content_repair_graph_json() -> Value {
    json!({
        "schema": "elastos.content.repair-graph/v1",
        "policy": "not_reported",
        "requested_kind": "auto",
        "status": "not_reported",
        "supported_import_fallbacks": ["object_manifest", "exact_bytes"],
        "refuses_exact_fallback_for_arbitrary_dag": true,
    })
}

fn is_default_content_repair_graph_json(value: &Value) -> bool {
    value == &default_content_repair_graph_json()
}

fn local_abuse_controls_json() -> Value {
    json!({
        "schema": CONTENT_ABUSE_CONTROLS_SCHEMA,
        "policy": "local_content_backend",
        "scope": "content-availability",
        "enforced": false,
        "throttled": false,
        "attempted_operations": 0,
        "failed_operations": 0,
    })
}

fn provider_abuse_controls_json() -> Value {
    json!({
        "schema": CONTENT_ABUSE_CONTROLS_SCHEMA,
        "policy": "availability_provider_not_reported",
        "scope": "content-availability",
        "enforced": false,
        "throttled": false,
        "attempted_operations": 0,
        "failed_operations": 0,
    })
}

fn default_content_abuse_controls_json() -> Value {
    provider_abuse_controls_json()
}

#[derive(Debug, Clone, Copy, Default)]
struct ContentAccountingObservation {
    files: Option<u64>,
    bytes: Option<u64>,
}

fn content_accounting_observation_from_publish_request(
    request: &Value,
) -> ContentAccountingObservation {
    match request.get("kind").and_then(|kind| kind.as_str()) {
        Some("file") => ContentAccountingObservation {
            files: Some(1),
            bytes: request.get("data").and_then(decoded_base64_len),
        },
        Some("directory") => content_accounting_observation_from_files(
            request.get("files").unwrap_or(&Value::Array(Vec::new())),
        ),
        _ => ContentAccountingObservation::default(),
    }
}

fn content_accounting_observation_from_files(files: &Value) -> ContentAccountingObservation {
    let Some(files) = files.as_array() else {
        return ContentAccountingObservation::default();
    };
    let mut bytes = 0_u64;
    for file in files {
        let Some(file_bytes) = file.get("data").and_then(decoded_base64_len) else {
            return ContentAccountingObservation {
                files: Some(files.len() as u64),
                bytes: None,
            };
        };
        bytes = bytes.saturating_add(file_bytes);
    }
    ContentAccountingObservation {
        files: Some(files.len() as u64),
        bytes: Some(bytes),
    }
}

fn content_accounting_observation_from_value(accounting: &Value) -> ContentAccountingObservation {
    ContentAccountingObservation {
        files: accounting.get("files").and_then(|value| value.as_u64()),
        bytes: accounting
            .get("content_bytes")
            .and_then(|value| value.as_u64()),
    }
}

fn admission_content_bytes_from_request(request: &Value) -> Option<u64> {
    ["estimated_content_bytes", "incoming_content_bytes"]
        .into_iter()
        .find_map(|field| request.get(field).and_then(|value| value.as_u64()))
        .or_else(|| {
            request
                .get("accounting")
                .and_then(|accounting| accounting.get("content_bytes"))
                .and_then(|value| value.as_u64())
        })
        .or_else(|| {
            request
                .get("local")
                .and_then(|local| local.get("accounting"))
                .and_then(|accounting| accounting.get("content_bytes"))
                .and_then(|value| value.as_u64())
        })
}

fn decoded_base64_len(value: &Value) -> Option<u64> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str()?)
        .ok()
        .map(|bytes| bytes.len() as u64)
}

fn content_accounting_json(
    source: &str,
    observation: ContentAccountingObservation,
    replicas: u32,
) -> Value {
    content_accounting_json_with_storage_quota(
        source,
        observation,
        replicas,
        default_storage_quota_json(),
    )
}

fn content_accounting_json_with_storage_quota(
    source: &str,
    observation: ContentAccountingObservation,
    replicas: u32,
    storage_quota: Value,
) -> Value {
    let replica_bytes = observation
        .bytes
        .map(|bytes| bytes.saturating_mul(replicas as u64));
    json!({
        "schema": CONTENT_ACCOUNTING_SCHEMA,
        "policy": "content_provider_local_accounting",
        "scope": "content-availability",
        "source": source,
        "observed": observation.files.is_some() || observation.bytes.is_some(),
        "files": observation.files,
        "content_bytes": observation.bytes,
        "replicas": replicas,
        "replica_bytes_estimate": replica_bytes,
        "storage_quota": storage_quota,
    })
}

fn default_storage_quota_json() -> Value {
    json!({
        "schema": CONTENT_STORAGE_QUOTA_SCHEMA,
        "policy": "observed_not_enforced",
        "scope": "content-availability",
        "enforced": false,
        "status": "observed_not_enforced",
        "reason": "content-provider records local storage accounting; no principal storage quota was requested",
        "federated_quota_ledger_policy": federated_quota_ledger_policy_json(
            "observed_not_enforced",
            "observed_not_enforced",
            true,
            false,
            false,
        ),
    })
}

fn content_storage_accounting_entry_from_receipt(
    receipt: &AvailabilityReceipt,
) -> ContentStorageAccountingEntry {
    let observation = content_accounting_observation_from_value(&receipt.accounting);
    let replica_bytes_estimate = receipt
        .accounting
        .get("replica_bytes_estimate")
        .and_then(|value| value.as_u64())
        .or_else(|| {
            observation
                .bytes
                .map(|bytes| bytes.saturating_mul(receipt.replicas as u64))
        });
    let storage_quota = receipt
        .accounting
        .get("storage_quota")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "schema": CONTENT_STORAGE_QUOTA_SCHEMA,
                "policy": "not_reported",
                "scope": "content-availability",
                "enforced": false,
                "status": "not_reported",
            })
        });

    ContentStorageAccountingEntry {
        schema: CONTENT_STORAGE_ACCOUNTING_ENTRY_SCHEMA.to_string(),
        cid: receipt.cid.clone(),
        uri: receipt.uri.clone(),
        object_did: receipt.object_did.clone(),
        principal_did: receipt.publisher_did.clone(),
        provider: receipt.provider.clone(),
        policy: receipt.policy.clone(),
        status: receipt.status.clone(),
        source: receipt
            .accounting
            .get("source")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string(),
        files: observation.files,
        content_bytes: observation.bytes,
        replicas: receipt.replicas,
        replica_bytes_estimate,
        quota: receipt.quota.clone(),
        storage_quota,
        recorded_at: receipt.checked_at,
    }
}

fn default_content_accounting_json() -> Value {
    content_accounting_json("legacy_receipt", ContentAccountingObservation::default(), 0)
}

fn availability_quota_status(quota: &Value) -> String {
    quota
        .get("status")
        .and_then(|value| value.as_str())
        .or_else(|| {
            quota
                .get("policy")
                .and_then(|value| value.as_str())
                .filter(|policy| *policy == "not_enforced")
        })
        .unwrap_or("unknown")
        .to_string()
}

fn repair_worker_json(scheduled: bool) -> Value {
    json!({
        "scheduled": scheduled,
        "status": if scheduled { "needed" } else { "not_scheduled" },
    })
}

fn provider_ok(data: Value) -> Value {
    json!({
        "status": "ok",
        "data": data,
    })
}

fn provider_error(code: &str, message: &str) -> Value {
    json!({
        "status": "error",
        "code": code,
        "message": message,
    })
}

fn validate_import_exact_invocation(request: &Value) -> Result<(), ProviderError> {
    let runtime = runtime_invocation_object(
        request,
        "content import_exact requires Runtime provider invocation metadata",
    )?;
    validate_runtime_invocation_fields(
        runtime,
        "content import_exact",
        &[
            ("schema", "elastos.provider.invocation/v1"),
            ("source", "carrier-availability"),
            ("target", "content"),
            ("op", "import_exact"),
            ("transport", "carrier-provider-plane"),
        ],
    )?;
    if !matches!(
        runtime.get("transfer").and_then(|value| value.as_str()),
        Some("json" | "bytes" | "stream")
    ) {
        return Err(ProviderError::Provider(
            "content import_exact transfer must be json, bytes, or stream".into(),
        ));
    }
    Ok(())
}

fn validate_import_object_invocation(request: &Value) -> Result<(), ProviderError> {
    let runtime = runtime_invocation_object(
        request,
        "content import_object requires Runtime provider invocation metadata",
    )?;
    validate_runtime_invocation_fields(
        runtime,
        "content import_object",
        &[
            ("schema", "elastos.provider.invocation/v1"),
            ("source", "carrier-availability"),
            ("target", "content"),
            ("op", "import_object"),
            ("transport", "carrier-provider-plane"),
            ("transfer", "json"),
        ],
    )
}

fn validate_repair_worker_invocation(request: &Value) -> Result<(), ProviderError> {
    let runtime = runtime_invocation_object(
        request,
        "content repair_worker requires Runtime provider invocation metadata",
    )?;
    validate_runtime_invocation_fields(
        runtime,
        "content repair_worker",
        &[
            ("schema", "elastos.provider.invocation/v1"),
            ("source", "content-provider"),
            ("target", "content"),
            ("op", "repair_worker"),
            ("transport", "runtime-local-provider-plane"),
            ("transfer", "json"),
        ],
    )
}

fn runtime_invocation_object<'a>(
    request: &'a Value,
    missing_message: &str,
) -> Result<&'a serde_json::Map<String, Value>, ProviderError> {
    request
        .get("_runtime_invocation")
        .and_then(|value| value.as_object())
        .ok_or_else(|| ProviderError::Provider(missing_message.to_string()))
}

fn validate_runtime_invocation_fields(
    runtime: &serde_json::Map<String, Value>,
    label: &str,
    fields: &[(&str, &str)],
) -> Result<(), ProviderError> {
    for (field, expected) in fields {
        let actual = runtime
            .get(*field)
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if actual != *expected {
            return Err(ProviderError::Provider(format!(
                "{label} runtime field {field} mismatch: expected {expected}, got {actual}"
            )));
        }
    }
    Ok(())
}

fn import_exact_payload_bytes(request: &Value) -> Result<Vec<u8>, ProviderError> {
    let bytes = if let Some(data) = request.get("data").and_then(|value| value.as_str()) {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|err| {
                ProviderError::Provider(format!("content import_exact data must be base64: {err}"))
            })?
    } else if let Some(stream) = request.get("stream") {
        provider_stream_payload_bytes(stream)?
    } else {
        return Err(ProviderError::Provider(
            "content import_exact requires data or stream".into(),
        ));
    };
    if bytes.len() > IMPORT_EXACT_MAX_BYTES {
        return Err(ProviderError::Provider(format!(
            "content import_exact payload exceeds {} bytes",
            IMPORT_EXACT_MAX_BYTES
        )));
    }
    Ok(bytes)
}

fn validate_import_object_payload_bounds(files: &Value) -> Result<(usize, usize), ProviderError> {
    let files = files.as_array().ok_or_else(|| {
        ProviderError::Provider("content import_object files must be an array".into())
    })?;
    if files.is_empty() {
        return Err(ProviderError::Provider(
            "content import_object requires at least one file".into(),
        ));
    }
    if files.len() > IMPORT_OBJECT_MAX_FILES {
        return Err(ProviderError::Provider(format!(
            "content import_object file count exceeds {IMPORT_OBJECT_MAX_FILES}"
        )));
    }
    let mut total_bytes = 0_usize;
    for file in files {
        let path = file
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let data = file
            .get("data")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ProviderError::Provider(format!(
                    "content import_object file {path} is missing base64 data"
                ))
            })?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|err| {
                ProviderError::Provider(format!(
                    "content import_object file {path} has invalid base64 data: {err}"
                ))
            })?;
        total_bytes = total_bytes.saturating_add(decoded.len());
        if total_bytes > IMPORT_EXACT_MAX_BYTES {
            return Err(ProviderError::Provider(format!(
                "content import_object payload exceeds {} bytes",
                IMPORT_EXACT_MAX_BYTES
            )));
        }
    }
    Ok((files.len(), total_bytes))
}

fn provider_stream_payload_bytes(stream: &Value) -> Result<Vec<u8>, ProviderError> {
    let object = stream.as_object().ok_or_else(|| {
        ProviderError::Provider("content import_exact stream must be an object".into())
    })?;
    let schema = object
        .get("schema")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if schema != "elastos.provider.stream/v1" {
        return Err(ProviderError::Provider(format!(
            "content import_exact stream schema mismatch: expected elastos.provider.stream/v1, got {schema}"
        )));
    }
    let encoding = object
        .get("encoding")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if encoding != "base64-chunks" {
        return Err(ProviderError::Provider(format!(
            "content import_exact stream encoding mismatch: expected base64-chunks, got {encoding}"
        )));
    }
    let chunks = object
        .get("chunks")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            ProviderError::Provider("content import_exact stream missing chunks".into())
        })?;
    let mut bytes = Vec::new();
    for (expected_index, chunk) in chunks.iter().enumerate() {
        let chunk = chunk.as_object().ok_or_else(|| {
            ProviderError::Provider("content import_exact stream chunk must be an object".into())
        })?;
        let index = chunk
            .get("index")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                ProviderError::Provider("content import_exact stream chunk missing index".into())
            })?;
        if index != expected_index as u64 {
            return Err(ProviderError::Provider(format!(
                "content import_exact stream chunk index mismatch: expected {expected_index}, got {index}"
            )));
        }
        let offset = chunk
            .get("offset")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                ProviderError::Provider("content import_exact stream chunk missing offset".into())
            })?;
        if offset != bytes.len() as u64 {
            return Err(ProviderError::Provider(format!(
                "content import_exact stream chunk {index} offset mismatch: expected {}, got {offset}",
                bytes.len()
            )));
        }
        let encoded = chunk
            .get("data")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ProviderError::Provider("content import_exact stream chunk missing data".into())
            })?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|err| {
                ProviderError::Provider(format!(
                    "content import_exact stream chunk has invalid base64: {err}"
                ))
            })?;
        if bytes.len().saturating_add(decoded.len()) > IMPORT_EXACT_MAX_BYTES {
            return Err(ProviderError::Provider(format!(
                "content import_exact payload exceeds {} bytes",
                IMPORT_EXACT_MAX_BYTES
            )));
        }
        if let Some(length) = chunk.get("length").and_then(|value| value.as_u64()) {
            if length != decoded.len() as u64 {
                return Err(ProviderError::Provider(format!(
                    "content import_exact stream chunk {index} length {length} does not match decoded length {}",
                    decoded.len()
                )));
            }
        }
        bytes.extend_from_slice(&decoded);
    }
    if let Some(total_bytes) = object.get("total_bytes").and_then(|value| value.as_u64()) {
        if total_bytes != bytes.len() as u64 {
            return Err(ProviderError::Provider(format!(
                "content import_exact stream total_bytes {total_bytes} does not match decoded length {}",
                bytes.len()
            )));
        }
    }
    Ok(bytes)
}

fn parse_availability_provider_response(
    response: &Value,
    requested_policy: &str,
    local: &AvailabilityOutcome,
    requirements: &AvailabilityRequirements,
) -> AvailabilityOutcome {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("availability provider returned an error")
            .to_string();
        return AvailabilityOutcome::repair_needed(
            "availability-provider",
            requested_policy,
            local.replicas,
            message,
        );
    }

    let data = response.get("data").unwrap_or(response);
    let availability = data.get("availability").unwrap_or(data);
    let provider = availability
        .get("provider")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("availability-provider");
    let policy = availability
        .get("policy")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(requested_policy);
    let replicas = availability
        .get("replicas")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(local.replicas);
    let status = availability
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    match status {
        "network_available" if replicas > 0 => {
            match validate_network_availability_claim(status, replicas, availability, requirements)
            {
                Ok(policy_metadata) => AvailabilityOutcome {
                    provider: provider.to_string(),
                    policy: policy.to_string(),
                    status: status.to_string(),
                    replicas,
                    reason: None,
                    peer_selection: policy_metadata.0,
                    quota: policy_metadata.1,
                    repair_worker: policy_metadata.2,
                    storage_market: policy_metadata.3,
                    repair_graph: policy_metadata.4,
                    abuse_controls: policy_metadata.5,
                },
                Err(reason) => {
                    AvailabilityOutcome::repair_needed(provider, policy, local.replicas, reason)
                }
            }
        }
        "carrier_announced" if replicas > 0 => {
            match validate_network_availability_claim(status, replicas, availability, requirements)
            {
                Ok(policy_metadata) => AvailabilityOutcome {
                    provider: provider.to_string(),
                    policy: policy.to_string(),
                    status: status.to_string(),
                    replicas,
                    reason: availability
                        .get("reason")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                    peer_selection: policy_metadata.0,
                    quota: policy_metadata.1,
                    repair_worker: policy_metadata.2,
                    storage_market: policy_metadata.3,
                    repair_graph: policy_metadata.4,
                    abuse_controls: policy_metadata.5,
                },
                Err(reason) => {
                    AvailabilityOutcome::repair_needed(provider, policy, local.replicas, reason)
                }
            }
        }
        "repair_needed" => AvailabilityOutcome::repair_needed(
            provider,
            policy,
            replicas,
            availability
                .get("reason")
                .and_then(|value| value.as_str())
                .unwrap_or("availability provider reported repair_needed")
                .to_string(),
        ),
        "network_available" => AvailabilityOutcome::repair_needed(
            provider,
            policy,
            local.replicas,
            "availability provider reported network_available without replicas".to_string(),
        ),
        "carrier_announced" => AvailabilityOutcome::repair_needed(
            provider,
            policy,
            local.replicas,
            "Carrier availability announcement did not include a local replica".to_string(),
        ),
        _ => AvailabilityOutcome::repair_needed(
            provider,
            policy,
            local.replicas,
            "availability provider returned an unsupported status".to_string(),
        ),
    }
}

fn validate_network_availability_claim(
    status: &str,
    replicas: u32,
    availability: &Value,
    requirements: &AvailabilityRequirements,
) -> Result<(Value, Value, Value, Value, Value, Value), String> {
    if replicas < requirements.min_replicas {
        return Err(format!(
            "availability provider reported {replicas} replicas below required {}",
            requirements.min_replicas
        ));
    }
    if let Some(max_replicas) = requirements.max_replicas {
        if replicas > max_replicas {
            return Err(format!(
                "availability provider reported {replicas} replicas over quota {max_replicas}"
            ));
        }
    }

    let peer_selection = availability
        .get("peer_selection")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| "network availability requires peer_selection metadata".to_string())?;
    let quota = availability
        .get("quota")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| "network availability requires quota metadata".to_string())?;
    let repair_worker = availability
        .get("repair_worker")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| "network availability requires repair_worker metadata".to_string())?;
    let abuse_controls = availability
        .get("abuse_controls")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(provider_abuse_controls_json);
    let storage_market = availability
        .get("storage_market")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(default_content_storage_market_json);
    let repair_graph = availability
        .get("repair_graph")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(default_content_repair_graph_json);

    let peer_selection_mode = peer_selection
        .get("mode")
        .or_else(|| peer_selection.get("strategy"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());
    if peer_selection_mode.is_none() {
        return Err("network availability peer_selection requires mode or strategy".to_string());
    }

    let quota_policy = quota
        .get("policy")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());
    if quota_policy.is_none() {
        return Err("network availability quota requires policy".to_string());
    }
    if let Some(max_replicas) = quota
        .get("max_replicas")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
    {
        if replicas > max_replicas {
            return Err(format!(
                "availability provider reported {replicas} replicas above quota max_replicas {max_replicas}"
            ));
        }
    }

    let repair_status = repair_worker
        .get("status")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());
    if repair_status.is_none() {
        return Err("network availability repair_worker requires status".to_string());
    }

    let live_proof = peer_selection
        .get("live_multi_peer_proof")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if replicas > 1 && !live_proof {
        return Err("multi-peer availability requires live_multi_peer_proof=true".to_string());
    }
    if requirements.require_live_multi_peer_proof && !live_proof {
        return Err("availability requirements demand live_multi_peer_proof=true".to_string());
    }
    if status == "carrier_announced" {
        let topic = peer_selection
            .get("topic")
            .or_else(|| availability.get("topic"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty());
        if topic.is_none() {
            return Err("Carrier availability announcement requires a topic".to_string());
        }
    }

    Ok((
        peer_selection,
        quota,
        repair_worker,
        storage_market,
        repair_graph,
        abuse_controls,
    ))
}

fn provider_response_cid(response: &Value) -> Result<String, ProviderError> {
    provider_response_ok(response, "content publish")?;
    response
        .get("data")
        .and_then(|data| data.get("cid"))
        .and_then(|cid| cid.as_str())
        .map(str::to_string)
        .ok_or_else(|| ProviderError::Provider("content backend response missing cid".into()))
}

fn content_response_cid(response: &Value) -> anyhow::Result<String> {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("content publish failed: {message}");
    }
    response
        .get("data")
        .and_then(|data| data.get("cid"))
        .and_then(|cid| cid.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("No CID in content provider response"))
}

fn provider_response_ok(response: &Value, operation: &str) -> Result<(), ProviderError> {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        return Err(ProviderError::Provider(format!(
            "{operation} failed: {message}"
        )));
    }
    Ok(())
}

fn provider_response_data(response: &Value, provider_name: &str) -> Result<String, ProviderError> {
    response
        .get("data")
        .and_then(|data| data.get("data"))
        .and_then(|data| data.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderError::Provider(format!("{provider_name} response missing base64 data"))
        })
}

fn provider_response_stream(response: &Value, provider_name: &str) -> Result<Value, ProviderError> {
    response
        .get("data")
        .and_then(|data| data.get("stream"))
        .cloned()
        .ok_or_else(|| {
            ProviderError::Provider(format!("{provider_name} response missing stream payload"))
        })
}

fn provider_response_payload(
    response: &Value,
    provider_name: &str,
    transfer: &ContentFetchTransfer,
) -> Result<ContentFetchPayload, ProviderError> {
    match transfer.transfer {
        ProviderTransfer::Stream => Ok(ContentFetchPayload::Stream(provider_response_stream(
            response,
            provider_name,
        )?)),
        _ => Ok(ContentFetchPayload::Bytes(provider_response_data(
            response,
            provider_name,
        )?)),
    }
}

fn provider_transfer_value(response: &Value) -> Option<Value> {
    response.get("_runtime_transfer").cloned()
}

fn is_valid_cid(value: &str) -> bool {
    cid::Cid::try_from(value).is_ok()
}

fn validate_content_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("content fetch path must be relative".to_string());
    }
    if path.contains('\\') || path.contains('\0') {
        return Err("content fetch path contains invalid characters".to_string());
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("content fetch path contains an invalid segment".to_string());
        }
    }
    Ok(())
}

fn with_directory_object_manifest(
    files: Value,
    kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    links: Option<&Value>,
) -> Result<Value, ProviderError> {
    let mut files = files
        .as_array()
        .cloned()
        .ok_or_else(|| ProviderError::Provider("files must be an array".into()))?;
    let manifest = directory_object_manifest(&files, kind, object_did, publisher_did, links)?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|err| {
        ProviderError::Provider(format!("content object manifest encode failed: {err}"))
    })?;
    files.push(json!({
        "path": OBJECT_MANIFEST_PATH,
        "data": base64::engine::general_purpose::STANDARD.encode(manifest_bytes),
    }));
    sort_directory_entries(&mut files)?;
    Ok(Value::Array(files))
}

fn directory_object_manifest(
    files: &[Value],
    kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    links: Option<&Value>,
) -> Result<ContentObjectManifest, ProviderError> {
    let kind = validate_content_object_kind(kind)?;
    let links = parse_content_object_links(links)?;
    let mut seen_paths = BTreeSet::new();
    let mut object_files = Vec::with_capacity(files.len());
    let mut sealed_object = None;
    for file in files {
        let path = file
            .get("path")
            .and_then(|path| path.as_str())
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                ProviderError::Provider("directory publish file is missing path".into())
            })?;
        if path == OBJECT_MANIFEST_PATH {
            return Err(ProviderError::Provider(format!(
                "{OBJECT_MANIFEST_PATH} is reserved for the content object manifest"
            )));
        }
        validate_content_path(path).map_err(ProviderError::Provider)?;
        if !seen_paths.insert(path.to_string()) {
            return Err(ProviderError::Provider(format!(
                "duplicate directory publish path: {path}"
            )));
        }
        let data = file
            .get("data")
            .and_then(|data| data.as_str())
            .ok_or_else(|| {
                ProviderError::Provider(format!(
                    "directory publish file {path} is missing base64 data"
                ))
            })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|err| {
                ProviderError::Provider(format!(
                    "directory publish file {path} has invalid base64 data: {err}"
                ))
            })?;
        if kind == "sealed" && path == SEALED_OBJECT_PATH {
            let sealed: SealedObjectV1 = serde_json::from_slice(&bytes).map_err(|err| {
                ProviderError::Provider(format!(
                    "sealed content object has invalid {SEALED_OBJECT_PATH}: {err}"
                ))
            })?;
            validate_sealed_object_descriptor(&sealed)?;
            sealed_object = Some(sealed);
        }
        object_files.push(ContentObjectFile {
            path: path.to_string(),
            sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
            size: bytes.len() as u64,
        });
    }
    object_files.sort_by(|a, b| a.path.cmp(&b.path));
    if kind == "sealed" {
        let sealed_object = sealed_object.ok_or_else(|| {
            ProviderError::Provider(format!(
                "sealed content object requires {SEALED_OBJECT_PATH}"
            ))
        })?;
        validate_sealed_content_links(&sealed_object, &links)?;
    }

    let mut hasher = sha2::Sha256::new();
    for file in &object_files {
        hasher.update(file.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.size.to_string().as_bytes());
        hasher.update(b"\0");
    }

    Ok(ContentObjectManifest {
        schema: OBJECT_MANIFEST_SCHEMA.to_string(),
        kind,
        content_digest: format!("sha256:{:x}", hasher.finalize()),
        files: object_files,
        links,
        object_did: object_did.map(str::to_string),
        publisher_did: publisher_did.map(str::to_string),
    })
}

fn validate_content_object_kind(kind: &str) -> Result<String, ProviderError> {
    match kind {
        "capsule" | "directory" | "document" | "protected-content" | "release" | "sealed"
        | "share" | "site" => Ok(kind.to_string()),
        _ => Err(ProviderError::Provider(format!(
            "unsupported content object kind: {kind}"
        ))),
    }
}

fn parse_content_object_links(
    links: Option<&Value>,
) -> Result<Vec<ContentObjectLink>, ProviderError> {
    let Some(links) = links else {
        return Ok(Vec::new());
    };
    let links = links
        .as_array()
        .ok_or_else(|| ProviderError::Provider("content object links must be an array".into()))?;
    let mut parsed = Vec::with_capacity(links.len());
    let mut seen = BTreeSet::new();
    for link in links {
        let rel = link
            .get("rel")
            .and_then(|rel| rel.as_str())
            .filter(|rel| !rel.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content object link is missing rel".into()))?;
        validate_content_object_link_rel(rel)?;
        let cid = link
            .get("cid")
            .and_then(|cid| cid.as_str())
            .filter(|cid| !cid.trim().is_empty())
            .ok_or_else(|| ProviderError::Provider("content object link is missing cid".into()))?;
        cid::Cid::try_from(cid).map_err(|err| {
            ProviderError::Provider(format!("invalid content object link cid: {err}"))
        })?;
        if !seen.insert((rel.to_string(), cid.to_string())) {
            return Err(ProviderError::Provider(format!(
                "duplicate content object link: {rel} {cid}"
            )));
        }
        parsed.push(ContentObjectLink {
            rel: rel.to_string(),
            cid: cid.to_string(),
        });
    }
    parsed.sort_by(|a, b| a.rel.cmp(&b.rel).then_with(|| a.cid.cmp(&b.cid)));
    Ok(parsed)
}

fn validate_sealed_object_descriptor(object: &SealedObjectV1) -> Result<(), ProviderError> {
    if object.schema != SEALED_OBJECT_SCHEMA {
        return Err(ProviderError::Provider(
            "sealed content object schema is unsupported".to_string(),
        ));
    }
    validate_linked_cid(&object.payload_cid, "payload_cid")?;
    validate_linked_cid(&object.rights_policy_cid, "rights_policy_cid")?;
    validate_linked_cid(&object.availability_receipt_cid, "availability_receipt_cid")?;
    require_field(&object.key_envelope.scheme, "key_envelope.scheme")?;
    require_field(&object.key_envelope.kid, "key_envelope.kid")?;
    require_field(&object.key_envelope.wrapped_cek, "key_envelope.wrapped_cek")?;
    require_field(&object.key_envelope.policy_hash, "key_envelope.policy_hash")?;
    validate_protected_content_key_envelope_algorithms(&object.key_envelope.algorithms)
        .map_err(|err| ProviderError::Provider(format!("sealed content object {err}")))?;
    require_field(
        &object.viewer.required_interface,
        "viewer.required_interface",
    )
}

fn validate_sealed_content_links(
    object: &SealedObjectV1,
    links: &[ContentObjectLink],
) -> Result<(), ProviderError> {
    require_link(links, "payload", &object.payload_cid)?;
    require_link(links, "rights.policy", &object.rights_policy_cid)?;
    require_link(
        links,
        "availability.receipt",
        &object.availability_receipt_cid,
    )?;
    if !links.iter().any(|link| link.rel == "provenance") {
        return Err(ProviderError::Provider(
            "sealed content object requires provenance link".to_string(),
        ));
    }
    Ok(())
}

fn require_link(links: &[ContentObjectLink], rel: &str, cid: &str) -> Result<(), ProviderError> {
    if links.iter().any(|link| link.rel == rel && link.cid == cid) {
        Ok(())
    } else {
        Err(ProviderError::Provider(format!(
            "sealed content object requires {rel} link to {cid}"
        )))
    }
}

fn validate_linked_cid(value: &str, field: &str) -> Result<(), ProviderError> {
    require_field(value, field)?;
    cid::Cid::try_from(value)
        .map(|_| ())
        .map_err(|err| ProviderError::Provider(format!("invalid sealed object {field}: {err}")))
}

fn require_field(value: &str, field: &str) -> Result<(), ProviderError> {
    if value.trim().is_empty() {
        Err(ProviderError::Provider(format!(
            "sealed content object {field} is required"
        )))
    } else {
        Ok(())
    }
}

fn validate_content_object_link_rel(rel: &str) -> Result<(), ProviderError> {
    if rel.len() > 64 {
        return Err(ProviderError::Provider(
            "content object link rel is too long".into(),
        ));
    }
    if !rel.bytes().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_' || b == b'.'
    }) {
        return Err(ProviderError::Provider(
            "content object link rel must use lowercase ASCII, digits, '-', '_', or '.'".into(),
        ));
    }
    Ok(())
}

fn sort_directory_entries(files: &mut [Value]) -> Result<(), ProviderError> {
    for file in files.iter() {
        file.get("path")
            .and_then(|path| path.as_str())
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                ProviderError::Provider("directory publish file is missing path".into())
            })?;
    }
    files.sort_by(|a, b| {
        let a = a.get("path").and_then(|path| path.as_str()).unwrap_or("");
        let b = b.get("path").and_then(|path| path.as_str()).unwrap_or("");
        a.cmp(b)
    });
    Ok(())
}

async fn materialize_data_capsule(
    registry: &ProviderRegistry,
    cid: &str,
    manifest: &elastos_common::CapsuleManifest,
    manifest_bytes: &[u8],
    capsule_dir: &Path,
) -> anyhow::Result<()> {
    let object_manifest_bytes =
        fetch_bytes_via_provider(registry, cid, Some(OBJECT_MANIFEST_PATH))
            .await
            .map_err(|err| {
                anyhow::anyhow!(
                    "published data capsule {cid} is missing {OBJECT_MANIFEST_PATH}; republish it through content availability: {err}"
                )
            })?;
    let object_manifest = parse_content_object_manifest(cid, &object_manifest_bytes)?;

    write_materialized_file(capsule_dir, OBJECT_MANIFEST_PATH, &object_manifest_bytes).await?;

    let mut saw_capsule_manifest = false;
    for file in &object_manifest.files {
        validate_content_path(&file.path).map_err(|err| anyhow::anyhow!("{err}"))?;
        if file.path == OBJECT_MANIFEST_PATH {
            anyhow::bail!("{OBJECT_MANIFEST_PATH} cannot appear inside its own file list");
        }

        let bytes = if file.path == "capsule.json" {
            saw_capsule_manifest = true;
            manifest_bytes.to_vec()
        } else {
            fetch_bytes_via_provider(registry, cid, Some(&file.path)).await?
        };
        verify_content_object_file(cid, file, &bytes)?;
        write_materialized_file(capsule_dir, &file.path, &bytes).await?;
    }

    if !saw_capsule_manifest {
        anyhow::bail!("published data capsule {cid} object manifest is missing capsule.json");
    }

    let entrypoint_path = capsule_dir.join(&manifest.entrypoint);
    if !entrypoint_path.is_file() {
        anyhow::bail!(
            "Data capsule entrypoint '{}' missing after content materialization from CID {}",
            manifest.entrypoint,
            cid
        );
    }

    Ok(())
}

pub fn verify_content_object_file(
    cid: &str,
    file: &ContentObjectFile,
    bytes: &[u8],
) -> anyhow::Result<()> {
    if file.size != bytes.len() as u64 {
        anyhow::bail!(
            "content object file size mismatch for {}/{}: expected {}, got {}",
            cid,
            file.path,
            file.size,
            bytes.len()
        );
    }
    let actual_hash = format!("{:x}", sha2::Sha256::digest(bytes));
    if file.sha256 != actual_hash {
        anyhow::bail!(
            "content object file hash mismatch for {}/{}",
            cid,
            file.path
        );
    }
    Ok(())
}

async fn write_materialized_file(base: &Path, rel_path: &str, bytes: &[u8]) -> anyhow::Result<()> {
    validate_content_path(rel_path).map_err(|err| anyhow::anyhow!("{err}"))?;
    let path = base.join(rel_path);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, bytes).await?;
    Ok(())
}

fn append_jsonl<T: Serialize>(path: &Path, entry: &T) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, entry)
        .map_err(|err| ProviderError::Provider(format!("content receipt write failed: {err}")))?;
    file.write_all(b"\n")?;
    Ok(())
}

fn verify_signed_receipt(receipt: &SignedAvailabilityReceipt) -> Result<(), ProviderError> {
    let envelope = serde_json::to_vec(receipt)
        .map_err(|err| ProviderError::Provider(format!("content receipt encode failed: {err}")))?;
    crate::crypto::verify_signed_json_envelope_against_dids(
        &envelope,
        AVAILABILITY_RECEIPT_DOMAIN,
        std::slice::from_ref(&receipt.signer_did),
    )
    .map_err(|err| {
        ProviderError::Provider(format!("content receipt verification failed: {err}"))
    })?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    const TEST_CID: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    struct MockIpfsProvider {
        add_count: Mutex<usize>,
        added_files: Mutex<Vec<String>>,
        added_directories: Mutex<Vec<Vec<Value>>>,
        requests: Mutex<Vec<Value>>,
        cat_files: Mutex<HashMap<String, Vec<u8>>>,
        missing_paths: Mutex<Vec<String>>,
        pinned: Mutex<Vec<String>>,
        pin_error: Mutex<Option<String>>,
        unpinned: Mutex<Vec<String>>,
    }

    struct MockAvailabilityProvider {
        requests: Mutex<Vec<Value>>,
        response: Mutex<Value>,
    }

    #[async_trait]
    impl Provider for MockIpfsProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock ipfs provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-ipfs-provider"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            match request.get("op").and_then(|op| op.as_str()) {
                Some("add_directory") => {
                    *self.add_count.lock().await += 1;
                    self.added_directories
                        .lock()
                        .await
                        .push(request["files"].as_array().cloned().unwrap_or_default());
                    Ok(provider_ok(json!({ "cid": TEST_CID })))
                }
                Some("add_bytes") => {
                    let filename = request
                        .get("filename")
                        .and_then(|filename| filename.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.added_files.lock().await.push(filename);
                    Ok(provider_ok(json!({ "cid": TEST_CID })))
                }
                Some("cat") => {
                    let path = request
                        .get("path")
                        .and_then(|path| path.as_str())
                        .unwrap_or("")
                        .to_string();
                    if self
                        .missing_paths
                        .lock()
                        .await
                        .iter()
                        .any(|item| item == &path)
                    {
                        return Ok(provider_error("not_found", "mock content path missing"));
                    }
                    let bytes = self
                        .cat_files
                        .lock()
                        .await
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(|| b"hello content".to_vec());
                    Ok(provider_ok(json!({
                        "data": base64::engine::general_purpose::STANDARD.encode(bytes)
                    })))
                }
                Some("pin") => {
                    if let Some(message) = self.pin_error.lock().await.clone() {
                        return Ok(provider_error("pin_failed", &message));
                    }
                    let cid = request
                        .get("cid")
                        .and_then(|cid| cid.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.pinned.lock().await.push(cid);
                    Ok(provider_ok(json!({})))
                }
                Some("unpin") => {
                    let cid = request
                        .get("cid")
                        .and_then(|cid| cid.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.unpinned.lock().await.push(cid);
                    Ok(provider_ok(json!({})))
                }
                _ => Ok(provider_error("unsupported", "unsupported mock ipfs op")),
            }
        }
    }

    #[async_trait]
    impl Provider for MockAvailabilityProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock availability provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["availability"]
        }

        fn name(&self) -> &'static str {
            "mock-availability-provider"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            Ok(self.response.lock().await.clone())
        }
    }

    fn decode_test_stream_payload(stream: &Value) -> Vec<u8> {
        let chunks = stream["chunks"].as_array().unwrap();
        let mut bytes = Vec::new();
        for chunk in chunks {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(chunk["data"].as_str().unwrap())
                .unwrap();
            bytes.extend_from_slice(&decoded);
        }
        bytes
    }

    fn test_stream_payload(bytes: &[u8]) -> Value {
        json!({
            "schema": "elastos.provider.stream/v1",
            "encoding": "base64-chunks",
            "total_bytes": bytes.len(),
            "completed": true,
            "chunks": [{
                "index": 0,
                "offset": 0,
                "length": bytes.len(),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }],
        })
    }

    fn carrier_import_exact_invocation() -> Value {
        json!({
            "schema": "elastos.provider.invocation/v1",
            "source": "carrier-availability",
            "target": "content",
            "op": "import_exact",
            "capability": "provider:carrier-availability->content:import_exact",
            "transport": "carrier-provider-plane",
            "carrier": {
                "route": "connect_ticket",
                "peer_did": "did:key:zRemote",
                "timeout_ms": 5000
            },
            "transfer": "stream",
            "stream": {
                "schema": "elastos.provider.stream/v1",
                "encoding": "base64-chunks",
                "chunk_size": 65536
            },
            "range": null,
            "progress": null
        })
    }

    fn carrier_import_object_invocation() -> Value {
        json!({
            "schema": "elastos.provider.invocation/v1",
            "source": "carrier-availability",
            "target": "content",
            "op": "import_object",
            "capability": "provider:carrier-availability->content:import_object",
            "transport": "carrier-provider-plane",
            "carrier": {
                "route": "connect_ticket",
                "peer_did": "did:key:zRemote",
                "timeout_ms": 5000
            },
            "transfer": "json",
            "range": null,
            "progress": null
        })
    }

    async fn registry_with_content_and_ipfs() -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        registry_with_content_and_ipfs_with_alert_config(None).await
    }

    async fn registry_with_content_and_ipfs_with_alert_config(
        operator_alert_sink_config: Option<Value>,
    ) -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        registry_with_content_and_ipfs_with_configs(operator_alert_sink_config, None, None, None)
            .await
    }

    async fn registry_with_content_and_ipfs_with_federated_alert_exchange_config(
        federated_operator_alert_exchange_config: Option<Value>,
    ) -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        registry_with_content_and_ipfs_with_configs(
            None,
            None,
            None,
            federated_operator_alert_exchange_config,
        )
        .await
    }

    async fn registry_with_content_and_ipfs_with_configs(
        operator_alert_sink_config: Option<Value>,
        storage_market_admission_config: Option<Value>,
        external_repair_fleet_config: Option<Value>,
        federated_operator_alert_exchange_config: Option<Value>,
    ) -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        registry_with_content_and_ipfs_with_all_configs(
            operator_alert_sink_config,
            storage_market_admission_config,
            external_repair_fleet_config,
            federated_operator_alert_exchange_config,
            None,
            None,
        )
        .await
    }

    async fn registry_with_content_and_ipfs_with_quota_ledger_exchange_config(
        federated_quota_ledger_exchange_config: Option<Value>,
    ) -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        registry_with_content_and_ipfs_with_all_configs(
            None,
            None,
            None,
            None,
            federated_quota_ledger_exchange_config,
            None,
        )
        .await
    }

    async fn registry_with_content_and_ipfs_with_abuse_control_exchange_config(
        federated_abuse_control_exchange_config: Option<Value>,
    ) -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        registry_with_content_and_ipfs_with_all_configs(
            None,
            None,
            None,
            None,
            None,
            federated_abuse_control_exchange_config,
        )
        .await
    }

    async fn registry_with_content_and_ipfs_with_all_configs(
        operator_alert_sink_config: Option<Value>,
        storage_market_admission_config: Option<Value>,
        external_repair_fleet_config: Option<Value>,
        federated_operator_alert_exchange_config: Option<Value>,
        federated_quota_ledger_exchange_config: Option<Value>,
        federated_abuse_control_exchange_config: Option<Value>,
    ) -> (
        tempfile::TempDir,
        Arc<ProviderRegistry>,
        Arc<MockIpfsProvider>,
        Arc<ContentProvider>,
    ) {
        let data_dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let ipfs = Arc::new(MockIpfsProvider {
            add_count: Mutex::new(0),
            added_files: Mutex::new(Vec::new()),
            added_directories: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            cat_files: Mutex::new(HashMap::new()),
            missing_paths: Mutex::new(Vec::new()),
            pinned: Mutex::new(Vec::new()),
            pin_error: Mutex::new(None),
            unpinned: Mutex::new(Vec::new()),
        });
        registry
            .register_sub_provider("ipfs", ipfs.clone())
            .await
            .unwrap();
        let content = Arc::new(ContentProvider::new_with_external_configs(
            data_dir.path().to_path_buf(),
            Arc::downgrade(&registry),
            ContentProviderExternalConfigs {
                operator_alert_sink: operator_alert_sink_config,
                storage_market_admission: storage_market_admission_config,
                external_repair_fleet: external_repair_fleet_config,
                federated_operator_alert_exchange: federated_operator_alert_exchange_config,
                federated_quota_ledger_exchange: federated_quota_ledger_exchange_config,
                federated_abuse_control_exchange: federated_abuse_control_exchange_config,
            },
        ));
        registry.register(content.clone()).await;
        registry
            .register_sub_provider("content", content.clone())
            .await
            .unwrap();
        (data_dir, registry, ipfs, content)
    }

    fn spawn_operator_alert_sink() -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
            )
            .unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/content-alerts"), handle)
    }

    fn spawn_federated_operator_alert_exchange(
        response: Value,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/alerts/exchange"), handle)
    }

    fn signed_federated_quota_ledger_exchange_receipt(
        accepted: bool,
        reason: Option<&str>,
    ) -> Value {
        let (signing_key, _) = elastos_identity::derive_did(&[41_u8; 32]);
        let payload = json!({
            "schema": CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA,
            "provider": "test-quota-ledger",
            "scope": "content-availability",
            "exchange_id": "quota-exchange:test",
            "receipt_id": if accepted { "quota-receipt:accepted" } else { "quota-receipt:rejected" },
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "reason": reason,
            "checked_at": now_unix_secs(),
        });
        let canonical = serde_json::to_string(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &signing_key,
            CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_DOMAIN,
            canonical.as_bytes(),
        );
        json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        })
    }

    fn spawn_federated_quota_ledger_exchange_endpoint(
        response: Value,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/quota/exchange"), handle)
    }

    fn signed_federated_abuse_control_exchange_receipt(
        accepted: bool,
        reason: Option<&str>,
    ) -> Value {
        let (signing_key, _) = elastos_identity::derive_did(&[43_u8; 32]);
        let payload = json!({
            "schema": CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA,
            "provider": "test-abuse-control",
            "scope": "content-availability",
            "exchange_id": "abuse-control-exchange:test",
            "receipt_id": if accepted {
                "abuse-control-receipt:accepted"
            } else {
                "abuse-control-receipt:rejected"
            },
            "abuse_ledger_id": "abuse-ledger:test",
            "accepted": accepted,
            "status": if accepted { "accepted" } else { "rejected" },
            "reason": reason,
            "checked_at": now_unix_secs(),
        });
        let canonical = serde_json::to_string(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &signing_key,
            CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_DOMAIN,
            canonical.as_bytes(),
        );
        json!({
            "payload": payload,
            "signature": signature,
            "signer_did": signer_did,
        })
    }

    fn spawn_federated_abuse_control_exchange_endpoint(
        response: Value,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/abuse/exchange"), handle)
    }

    fn spawn_storage_market_admission_endpoint(
        response: Value,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/market/admission"), handle)
    }

    fn spawn_external_repair_fleet_endpoint(
        response: Value,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = response.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if http_request_complete(&request) {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}/repair/dispatch"), handle)
    }

    fn http_request_complete(request: &[u8]) -> bool {
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        request.len() >= header_end + 4 + content_length
    }

    async fn invoke_content_repair_worker(
        registry: &Arc<ProviderRegistry>,
        request: Value,
    ) -> Result<Value, ProviderError> {
        registry
            .invoke_provider(ProviderInvocation {
                source: "content-provider".to_string(),
                target: "content".to_string(),
                op: "repair_worker".to_string(),
                request,
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Local,
            })
            .await
    }

    #[tokio::test]
    async fn content_publish_wraps_ipfs_with_availability_status() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["cid"], TEST_CID);
        assert_eq!(response["data"]["uri"], format!("elastos://{TEST_CID}"));
        assert_eq!(response["data"]["availability"]["status"], "local_pinned");
        assert_eq!(
            response["data"]["availability"]["peer_selection"]["mode"],
            "single_local"
        );
        assert_eq!(
            response["data"]["availability"]["peer_selection"]["live_multi_peer_proof"],
            false
        );
        assert_eq!(
            response["data"]["availability"]["quota"]["policy"],
            "not_enforced"
        );
        assert_eq!(
            response["data"]["availability"]["repair_worker"]["scheduled"],
            false
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["schema"],
            AVAILABILITY_RECEIPT_SCHEMA
        );
        assert_eq!(response["data"]["receipt"]["payload"]["cid"], TEST_CID);
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "local_pinned"
        );
        assert!(response["data"]["receipt"]["signature"]
            .as_str()
            .is_some_and(|sig| !sig.is_empty()));
        assert!(response["data"]["receipt"]["signer_did"]
            .as_str()
            .is_some_and(|did| did.starts_with("did:key:z6Mk")));
        let signer_did = response["data"]["receipt"]["signer_did"]
            .as_str()
            .unwrap()
            .to_string();
        let signed_receipt = serde_json::to_vec(&response["data"]["receipt"]).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_receipt,
            AVAILABILITY_RECEIPT_DOMAIN,
            &[signer_did],
        )
        .unwrap();
        assert_eq!(*ipfs.add_count.lock().await, 1);
    }

    #[tokio::test]
    async fn content_publish_records_local_only_repair_task() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["repair_task"]["schema"],
            REPAIR_TASK_SCHEMA
        );
        assert_eq!(response["data"]["repair_task"]["cid"], TEST_CID);
        assert_eq!(response["data"]["repair_task"]["status"], "local_only");
        assert_eq!(
            response["data"]["repair_task"]["repair_worker"]["scheduled"],
            false
        );

        let status = content
            .send_raw(&json!({
                "op": "status",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["availability"]["repair_task"]["status"],
            "local_only"
        );
    }

    #[tokio::test]
    async fn content_import_exact_requires_runtime_provider_invocation() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let err = content
            .send_raw(&json!({
                "op": "import_exact",
                "cid": TEST_CID,
                "stream": test_stream_payload(b"hello content"),
            }))
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("requires Runtime provider invocation metadata"));
    }

    #[tokio::test]
    async fn content_import_exact_accepts_matching_cid_stream() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "import_exact",
                "cid": TEST_CID,
                "stream": test_stream_payload(b"hello content"),
                "_runtime_invocation": carrier_import_exact_invocation(),
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["cid"], TEST_CID);
        assert_eq!(response["data"]["availability"]["status"], "local_pinned");
        assert_eq!(
            response["data"]["availability"]["policy"],
            "carrier_exact_import"
        );
        assert_eq!(response["data"]["import"]["verified_cid"], true);
        assert_eq!(response["data"]["import"]["bytes"], 13);
        assert_eq!(
            ipfs.added_files.lock().await.as_slice(),
            ["content.bin".to_string()]
        );
    }

    #[tokio::test]
    async fn content_import_exact_rejects_cid_mismatch_and_unpins_import() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "import_exact",
                "cid": "QmRSEtAyq7Xgr5YCFVWuYsBdqbR5X9fJDsdpNQuvm9yaic",
                "stream": test_stream_payload(b"hello content"),
                "_runtime_invocation": carrier_import_exact_invocation(),
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "cid_mismatch");
        assert_eq!(
            ipfs.unpinned.lock().await.as_slice(),
            [TEST_CID.to_string()]
        );
    }

    #[tokio::test]
    async fn content_import_object_requires_runtime_provider_invocation() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let err = content
            .send_raw(&json!({
                "op": "import_object",
                "cid": TEST_CID,
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
            }))
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("requires Runtime provider invocation metadata"));
    }

    #[tokio::test]
    async fn content_import_object_reconstructs_manifest_directory() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "import_object",
                "cid": TEST_CID,
                "object_kind": "document",
                "object_did": "did:key:zObject",
                "publisher_did": "did:key:zPublisher",
                "links": [{"rel": "provenance", "cid": TEST_CID}],
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "_runtime_invocation": carrier_import_object_invocation(),
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["cid"], TEST_CID);
        assert_eq!(
            response["data"]["availability"]["policy"],
            "carrier_object_import"
        );
        assert_eq!(
            response["data"]["import"]["schema"],
            "elastos.content.import-object/v1"
        );
        assert_eq!(response["data"]["import"]["files"], 1);
        assert_eq!(response["data"]["import"]["verified_cid"], true);
        assert_eq!(
            response["data"]["receipt"]["payload"]["accounting"]["source"],
            "carrier_object_import"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["accounting"]["files"],
            1
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["accounting"]["content_bytes"],
            7
        );

        let directories = ipfs.added_directories.lock().await;
        assert_eq!(directories.len(), 1);
        let manifest_entry = directories[0]
            .iter()
            .find(|entry| entry["path"].as_str() == Some(OBJECT_MANIFEST_PATH))
            .expect("object manifest should be injected");
        let manifest_bytes = base64::engine::general_purpose::STANDARD
            .decode(manifest_entry["data"].as_str().unwrap())
            .unwrap();
        let manifest: ContentObjectManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest.kind, "document");
        assert_eq!(manifest.object_did.as_deref(), Some("did:key:zObject"));
        assert_eq!(
            manifest.publisher_did.as_deref(),
            Some("did:key:zPublisher")
        );
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.links[0].rel, "provenance");
    }

    #[tokio::test]
    async fn content_publish_uses_registered_availability_provider() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "elacity-supernode",
                    "policy": "smartweb_default",
                    "replicas": 3,
                    "peer_selection": {
                        "mode": "carrier_topic",
                        "strategy": "closest_peer",
                        "live_multi_peer_proof": true,
                        "peer_reputation_policy": {
                            "schema": "elastos.carrier.peer-reputation/v1",
                            "policy": "local_runtime_reputation",
                            "status": "local_history_applied",
                            "federation": {
                                "configured": false
                            }
                        },
                        "replicas": [{
                            "role": "remote",
                            "node_did": "did:key:zRemote",
                            "score": 94,
                            "selection_reason": "signed_announcement+endpoint_advertised+fresh+local_reputation_positive",
                            "local_reputation": {
                                "scope": "local_runtime",
                                "score_delta": 4,
                                "reason": "local_runtime_successes:1;failures:0"
                            },
                            "remote_receipt": {
                                "schema": "elastos.content.availability.receipt/v1",
                                "status": "local_pinned",
                                "verified": true,
                                "signer_did": "did:key:zRemoteContentProvider",
                                "quota": {
                                    "status": "within_quota"
                                },
                                "accounting": {
                                    "content_bytes": 7
                                },
                                "abuse_controls": {
                                    "policy": "carrier_provider_invocation_guardrail",
                                    "enforced": true,
                                    "attempted_operations": 1,
                                    "failed_operations": 0,
                                    "throttled": false
                                }
                            }
                        }]
                    },
                    "quota": {
                        "policy": "operator_default",
                        "status": "within_quota",
                        "enforced": true,
                        "max_replicas": 3
                    },
                    "repair_worker": {
                        "scheduled": false,
                        "status": "healthy"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "object_did": "did:key:z6Mkobject",
                "publisher_did": "did:key:z6Mkpublisher",
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["availability"]["status"],
            "network_available"
        );
        assert_eq!(
            response["data"]["availability"]["provider"],
            "elacity-supernode"
        );
        assert_eq!(response["data"]["availability"]["replicas"], 3);
        assert_eq!(
            response["data"]["availability"]["peer_selection"]["mode"],
            "carrier_topic"
        );
        assert_eq!(
            response["data"]["availability"]["peer_selection"]["live_multi_peer_proof"],
            true
        );
        assert_eq!(
            response["data"]["availability"]["quota"]["policy"],
            "operator_default"
        );
        assert_eq!(
            response["data"]["availability"]["repair_worker"]["status"],
            "healthy"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "network_available"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["provider"],
            "elacity-supernode"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["policy"],
            "smartweb_default"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["peer_selection"]["strategy"],
            "closest_peer"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["quota"]["max_replicas"],
            3
        );

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "ensure");
        assert_eq!(requests[0]["cid"], TEST_CID);
        assert_eq!(requests[0]["uri"], format!("elastos://{TEST_CID}"));
        assert_eq!(requests[0]["local"]["status"], "local_pinned");
        assert_eq!(requests[0]["requirements"]["min_replicas"], 1);
        assert_eq!(
            requests[0]["requirements"]["require_live_multi_peer_proof"],
            false
        );
        assert!(requests[0]["requirements"]["max_replicas"].is_null());
        assert_eq!(
            requests[0]["local"]["peer_selection"]["mode"],
            "single_local"
        );
        assert_eq!(requests[0]["object_did"], "did:key:z6Mkobject");
        assert_eq!(requests[0]["publisher_did"], "did:key:z6Mkpublisher");
        drop(requests);

        let dashboard = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();

        assert_eq!(dashboard["data"]["quota"]["by_status"]["within_quota"], 1);
        assert_eq!(dashboard["data"]["quota"]["enforced"], 1);
        assert_eq!(dashboard["data"]["proofs"]["live_multi_peer"], 1);
        assert_eq!(dashboard["data"]["proofs"]["remote_replicas"], 1);
        assert_eq!(dashboard["data"]["proofs"]["remote_receipts"], 1);
        assert_eq!(dashboard["data"]["proofs"]["verified_remote_receipts"], 1);
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replica_limit"].as_u64(),
            Some(AVAILABILITY_DASHBOARD_REMOTE_ROW_LIMIT as u64)
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas_truncated"],
            false
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["node_did"],
            "did:key:zRemote"
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["score"],
            94
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["local_reputation"]["scope"],
            "local_runtime"
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["peer_reputation_policy"]
                ["status"],
            "local_history_applied"
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["local_reputation"]
                ["score_delta"],
            4
        );
        assert_eq!(
            dashboard["data"]["proofs"]["peer_reputation_policy"]["by_status"]
                ["local_history_applied"],
            1
        );
        assert_eq!(
            dashboard["data"]["proofs"]["peer_reputation_policy"]["local_history_applied"],
            1
        );
        assert_eq!(
            dashboard["data"]["proofs"]["peer_reputation_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["remote_receipt"]["verified"],
            true
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["remote_receipt"]
                ["quota_status"],
            "within_quota"
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["remote_receipt"]
                ["content_bytes"],
            7
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["remote_receipt"]
                ["abuse_controls"]["policy"],
            "carrier_provider_invocation_guardrail"
        );
        assert_eq!(
            dashboard["data"]["proofs"]["recent_remote_replicas"][0]["remote_receipt"]
                ["abuse_controls"]["attempted_operations"],
            1
        );
        assert!(!dashboard.to_string().contains("connect_ticket"));
    }

    #[tokio::test]
    async fn content_repair_worker_requires_runtime_provider_invocation() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let err = content
            .send_raw(&json!({
                "op": "repair_worker",
                "force": true,
            }))
            .await
            .unwrap_err();

        assert!(err
            .to_string()
            .contains("repair_worker requires Runtime provider invocation metadata"));
    }

    #[tokio::test]
    async fn content_repair_worker_retries_queued_availability_task() {
        let (_data_dir, registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "elacity-supernode",
                    "policy": "smartweb_default",
                    "replicas": 2,
                    "peer_selection": {
                        "mode": "carrier_topic",
                        "live_multi_peer_proof": false
                    },
                    "quota": {
                        "policy": "operator_default",
                        "max_replicas": 2
                    },
                    "repair_worker": {
                        "scheduled": true,
                        "status": "healthy"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let publish = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "object_did": "did:key:z6Mkobject",
                "publisher_did": "did:key:z6Mkpublisher",
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(publish["status"], "ok");
        assert_eq!(publish["data"]["availability"]["status"], "repair_needed");
        assert_eq!(publish["data"]["repair_task"]["status"], "queued");
        assert_eq!(publish["data"]["repair_task"]["attempts"], 0);
        assert_eq!(
            publish["data"]["repair_task"]["repair_worker"]["scheduled"],
            true
        );

        *availability.response.lock().await = provider_ok(json!({
            "availability": {
                "status": "network_available",
                "provider": "elacity-supernode",
                "policy": "smartweb_default",
                "replicas": 2,
                "peer_selection": {
                    "mode": "carrier_topic",
                    "live_multi_peer_proof": true
                },
                "quota": {
                    "policy": "operator_default",
                    "max_replicas": 2
                },
                "repair_worker": {
                    "scheduled": true,
                    "status": "healthy"
                }
            }
        }));

        let worker = invoke_content_repair_worker(
            &registry,
            json!({
                "op": "repair_worker",
                "force": true,
            }),
        )
        .await
        .unwrap();

        assert_eq!(worker["status"], "ok");
        assert_eq!(worker["data"]["schema"], REPAIR_WORKER_RUN_SCHEMA);
        assert_eq!(worker["data"]["checked"], 1);
        assert_eq!(worker["data"]["repaired"], 1);
        assert_eq!(worker["data"]["failed"], 0);
        assert_eq!(
            worker["data"]["quota"]["policy"],
            "content_repair_worker_guardrail"
        );
        assert_eq!(
            worker["data"]["abuse_controls"]["schema"],
            REPAIR_WORKER_ABUSE_CONTROLS_SCHEMA
        );
        assert_eq!(
            worker["data"]["abuse_controls"]["runtime_invocation_required"],
            true
        );
        assert_eq!(
            worker["data"]["network_abuse_policy"]["schema"],
            CONTENT_NETWORK_ABUSE_POLICY_SCHEMA
        );
        assert_eq!(
            worker["data"]["network_abuse_policy"]["local_guardrails"]
                ["repair_worker_attempt_budget"],
            true
        );
        assert_eq!(
            worker["data"]["network_abuse_policy"]["network_federation"]["configured"],
            false
        );
        assert_eq!(
            worker["data"]["repair_fleet"]["schema"],
            REPAIR_FLEET_SCHEMA
        );
        assert_eq!(
            worker["data"]["repair_fleet"]["policy"],
            "single_runtime_provider_repair_fleet"
        );
        assert_eq!(worker["data"]["repair_fleet"]["checked"], 1);
        assert_eq!(
            worker["data"]["repair_fleet"]["production_federation"]["configured"],
            false
        );
        assert_eq!(
            worker["data"]["external_repair_fleet_policy"]["schema"],
            EXTERNAL_REPAIR_FLEET_POLICY_SCHEMA
        );
        assert_eq!(
            worker["data"]["external_repair_fleet_policy"]["external_fleet"]["configured"],
            false
        );
        assert_eq!(worker["data"]["results"][0]["cid"], TEST_CID);
        assert_eq!(worker["data"]["results"][0]["status"], "network_available");
        assert_eq!(ipfs.pinned.lock().await.as_slice(), &[TEST_CID.to_string()]);

        let status = content
            .send_raw(&json!({
                "op": "status",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["availability"]["status"],
            "network_available"
        );
        assert_eq!(
            status["data"]["availability"]["repair_task"]["status"],
            "healthy"
        );
        assert_eq!(
            status["data"]["availability"]["network_abuse_policy"]["schema"],
            CONTENT_NETWORK_ABUSE_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["availability"]["network_abuse_policy"]["network_federation"]
                ["cross_peer_rate_limit"],
            false
        );
        assert_eq!(status["data"]["availability"]["repair_task"]["attempts"], 1);

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["op"], "ensure");
        assert_eq!(requests[1]["op"], "ensure");
        assert_eq!(requests[1]["requirements"]["min_replicas"], 1);
        assert_eq!(
            requests[1]["requirements"]["require_live_multi_peer_proof"],
            false
        );
    }

    #[tokio::test]
    async fn content_repair_worker_reuses_three_replica_live_requirement_after_replica_loss() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability_response = |replicas| {
            provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "elacity-supernode",
                    "policy": "protected-content-replication/v1",
                    "replicas": replicas,
                    "peer_selection": {
                        "mode": "carrier_topic",
                        "live_multi_peer_proof": true
                    },
                    "quota": {
                        "policy": "operator_default"
                    },
                    "repair_worker": {
                        "scheduled": true,
                        "status": "healthy"
                    }
                }
            }))
        };
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(availability_response(3)),
        });
        registry.register(availability.clone()).await;

        let publish = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBQcm90ZWN0ZWQK"}],
                "object_did": "did:key:z6Mkprotected",
                "publisher_did": "did:key:z6Mkpublisher",
                "pin": true,
                "availability_requirements": {
                    "min_replicas": 3,
                    "require_live_multi_peer_proof": true
                }
            }))
            .await
            .unwrap();

        assert_eq!(
            publish["data"]["availability"]["status"],
            "network_available"
        );
        assert_eq!(
            publish["data"]["repair_task"]["requirements"]["min_replicas"],
            3
        );
        assert_eq!(
            publish["data"]["repair_task"]["requirements"]["require_live_multi_peer_proof"],
            true
        );
        assert!(publish["data"]["repair_task"]["requirements"]["max_replicas"].is_null());

        *availability.response.lock().await = availability_response(2);
        let worker = invoke_content_repair_worker(
            &registry,
            json!({
                "op": "repair_worker",
                "force": true,
                "include_healthy_check": true,
            }),
        )
        .await
        .unwrap();

        assert_eq!(worker["data"]["checked"], 1);
        assert_eq!(worker["data"]["repaired"], 0);
        assert_eq!(worker["data"]["failed"], 1);
        assert_eq!(worker["data"]["results"][0]["status"], "repair_needed");

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["requirements"]["min_replicas"], 3);
        assert_eq!(
            requests[1]["requirements"]["require_live_multi_peer_proof"],
            true
        );
        assert!(requests[1]["requirements"]["max_replicas"].is_null());
        drop(requests);

        let status = content
            .send_raw(&json!({
                "op": "status",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();
        assert_eq!(status["data"]["availability"]["status"], "repair_needed");
        assert_eq!(
            status["data"]["availability"]["repair_task"]["requirements"]["min_replicas"],
            3
        );
        assert_eq!(
            status["data"]["availability"]["repair_task"]["requirements"]
                ["require_live_multi_peer_proof"],
            true
        );
    }

    #[tokio::test]
    async fn content_repair_worker_dispatches_configured_external_repair_fleet() {
        let (url, handle) = spawn_external_repair_fleet_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "fleet_id": "fleet:supernode",
            "job_id": "repair:123",
            "receipt": {
                "schema": "elastos.test.external-repair-fleet.receipt/v1",
                "job_id": "repair:123"
            }
        }));
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs_with_configs(
            None,
            None,
            Some(json!({
                "url": url,
                "authorization": "Bearer fleet-test",
                "timeout_secs": 5,
            })),
            None,
        )
        .await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "repair_needed",
                    "provider": "carrier-availability",
                    "policy": "network_default",
                    "replicas": 1,
                    "reason": "remote peer could not pin content yet",
                    "peer_selection": {
                        "mode": "carrier_provider_replication",
                        "live_multi_peer_proof": false
                    },
                    "quota": {
                        "policy": "carrier_provider_quota",
                        "max_replicas": 2
                    },
                    "repair_worker": {
                        "scheduled": true,
                        "status": "queued"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let publish = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();
        assert_eq!(publish["data"]["repair_task"]["status"], "queued");

        *availability.response.lock().await = provider_ok(json!({
            "availability": {
                "status": "network_available",
                "provider": "elacity-supernode",
                "policy": "smartweb_default",
                "replicas": 2,
                "peer_selection": {
                    "mode": "carrier_topic",
                    "live_multi_peer_proof": true
                },
                "quota": {
                    "policy": "operator_default",
                    "max_replicas": 2
                },
                "repair_worker": {
                    "scheduled": true,
                    "status": "healthy"
                }
            }
        }));

        let worker = invoke_content_repair_worker(
            &registry,
            json!({
                "op": "repair_worker",
                "force": true,
            }),
        )
        .await
        .unwrap();

        assert_eq!(worker["status"], "ok");
        assert_eq!(worker["data"]["checked"], 1);
        assert_eq!(worker["data"]["repaired"], 1);
        assert_eq!(
            worker["data"]["external_repair_fleet_policy"]["status"],
            "external_repair_fleet_dispatch_configured"
        );
        assert_eq!(
            worker["data"]["external_repair_fleet_policy"]["run"]["external_dispatches"],
            1
        );
        assert_eq!(
            worker["data"]["external_repair_fleet_policy"]["run"]["external_dispatch_accepted"],
            1
        );
        assert_eq!(
            worker["data"]["external_repair_fleet_policy"]["external_fleet"]["configured"],
            true
        );
        assert_eq!(
            worker["data"]["results"][0]["external_repair_fleet_dispatch"]["schema"],
            EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA
        );
        assert_eq!(
            worker["data"]["results"][0]["external_repair_fleet_dispatch"]["job_id"],
            "repair:123"
        );
        assert_eq!(
            worker["data"]["results"][0]["external_repair_fleet_dispatch"]["client"]
                ["credential_exposed"],
            false
        );
        assert!(!worker.to_string().contains("fleet-test"));

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["external_repair_fleet_policy"]["external_fleet"]["configured"],
            true
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["fleet_history"]["external_repair_fleet_policy"]
                ["external_fleet"]["configured"],
            true
        );
        assert!(!status.to_string().contains("fleet-test"));

        let request = handle.join().unwrap();
        assert!(request.contains(EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer fleet-test")));
        assert!(!request
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("fleet-test"));
    }

    #[tokio::test]
    async fn content_external_repair_fleet_dispatch_accepts_endpoint_quorum() {
        let (url_a, handle_a) = spawn_external_repair_fleet_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "fleet_id": "fleet:a",
            "job_id": "repair:a",
            "receipt": {
                "schema": "elastos.test.external-repair-fleet.receipt/v1",
                "job_id": "repair:a"
            }
        }));
        let (url_b, handle_b) = spawn_external_repair_fleet_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "fleet_id": "fleet:b",
            "job_id": "repair:b",
            "receipt": {
                "schema": "elastos.test.external-repair-fleet.receipt/v1",
                "job_id": "repair:b"
            }
        }));
        let client = ContentExternalRepairFleetClient::from_config(json!({
            "quorum": 2,
            "endpoints": [
                {
                    "id": "repair-a",
                    "url": url_a,
                    "authorization": "Bearer repair-secret-a",
                    "timeout_secs": 5
                },
                {
                    "id": "repair-b",
                    "url": url_b,
                    "authorization": "Bearer repair-secret-b",
                    "timeout_secs": 5
                }
            ]
        }))
        .unwrap();

        let receipt = client
            .dispatch(&json!({
                "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA,
                "cid": TEST_CID,
                "requested_at": 1_700_000_000,
            }))
            .await
            .unwrap();

        assert_eq!(
            receipt["schema"],
            EXTERNAL_REPAIR_FLEET_DISPATCH_RECEIPT_SCHEMA
        );
        assert_eq!(receipt["accepted"], true);
        assert_eq!(receipt["status"], "accepted");
        assert_eq!(receipt["job_id"], "repair:a");
        assert_eq!(receipt["quorum"]["required"], 2);
        assert_eq!(receipt["quorum"]["endpoint_count"], 2);
        assert_eq!(receipt["quorum"]["accepted"], 2);
        assert_eq!(receipt["client"]["multi_endpoint"], true);
        assert_eq!(receipt["client"]["endpoint_count"], 2);
        assert!(!receipt.to_string().contains("repair-secret-a"));
        assert!(!receipt.to_string().contains("repair-secret-b"));

        let request_a = handle_a.join().unwrap();
        let request_b = handle_b.join().unwrap();
        assert!(request_a
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer repair-secret-a")));
        assert!(request_b
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer repair-secret-b")));
        assert!(request_a.contains(EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA));
        assert!(request_b.contains(EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA));
        assert!(!request_a
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("repair-secret-a"));
        assert!(!request_b
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("repair-secret-b"));
    }

    #[tokio::test]
    async fn content_external_repair_fleet_dispatch_rejects_endpoint_quorum_failure() {
        let (accepted_url, accepted_handle) = spawn_external_repair_fleet_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "fleet_id": "fleet:a",
            "job_id": "repair:a",
        }));
        let (rejected_url, rejected_handle) = spawn_external_repair_fleet_endpoint(json!({
            "accepted": false,
            "status": "rejected",
            "reason": "fleet capacity exhausted",
        }));
        let client = ContentExternalRepairFleetClient::from_config(json!({
            "quorum": 2,
            "endpoints": [
                {"id": "repair-a", "url": accepted_url, "timeout_secs": 5},
                {"id": "repair-b", "url": rejected_url, "timeout_secs": 5}
            ]
        }))
        .unwrap();

        let receipt = client
            .dispatch(&json!({
                "schema": EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA,
                "cid": TEST_CID,
                "requested_at": 1_700_000_000,
            }))
            .await
            .unwrap();

        assert_eq!(receipt["accepted"], false);
        assert_eq!(receipt["status"], "dispatch_failed");
        assert_eq!(receipt["quorum"]["required"], 2);
        assert_eq!(receipt["quorum"]["accepted"], 1);
        assert_eq!(receipt["quorum"]["rejected"], 1);
        assert!(receipt["reason"]
            .as_str()
            .unwrap()
            .contains("fleet capacity exhausted"));

        let accepted_request = accepted_handle.join().unwrap();
        let rejected_request = rejected_handle.join().unwrap();
        assert!(accepted_request.contains(EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA));
        assert!(rejected_request.contains(EXTERNAL_REPAIR_FLEET_DISPATCH_REQUEST_SCHEMA));
    }

    #[tokio::test]
    async fn content_repair_worker_enforces_attempt_budget() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "repair_needed",
                    "provider": "carrier-availability",
                    "policy": "network_default",
                    "replicas": 1,
                    "reason": "remote peer could not pin content yet",
                    "peer_selection": {
                        "mode": "carrier_provider_replication",
                        "live_multi_peer_proof": false
                    },
                    "quota": {
                        "policy": "carrier_provider_quota",
                        "max_replicas": 2
                    },
                    "repair_worker": {
                        "scheduled": true,
                        "status": "queued"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let publish = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();
        assert_eq!(publish["data"]["repair_task"]["status"], "queued");
        assert_eq!(publish["data"]["repair_task"]["attempts"], 0);

        let first = invoke_content_repair_worker(
            &registry,
            json!({
                "op": "repair_worker",
                "force": true,
                "max_attempts": 1,
            }),
        )
        .await
        .unwrap();
        assert_eq!(first["data"]["checked"], 1);
        assert_eq!(first["data"]["failed"], 1);
        assert_eq!(first["data"]["results"][0]["status"], "repair_needed");

        let second = invoke_content_repair_worker(
            &registry,
            json!({
                "op": "repair_worker",
                "force": true,
                "max_attempts": 1,
            }),
        )
        .await
        .unwrap();
        assert_eq!(second["data"]["checked"], 0);
        assert_eq!(
            second["data"]["abuse_controls"]["exhausted_attempts_skipped"],
            1
        );
        assert_eq!(
            second["data"]["network_abuse_policy"]["schema"],
            CONTENT_NETWORK_ABUSE_POLICY_SCHEMA
        );
        assert_eq!(
            second["data"]["network_abuse_policy"]["status"],
            "local_worker_throttled"
        );
        assert_eq!(
            second["data"]["repair_fleet"]["exhausted_attempts_skipped"],
            1
        );
        assert_eq!(
            second["data"]["repair_fleet"]["production_federation"]["external_workers"],
            false
        );
        assert_eq!(
            second["data"]["external_repair_fleet_policy"]["run"]["exhausted_attempts_skipped"],
            1
        );
        assert_eq!(
            second["data"]["external_repair_fleet_policy"]["external_fleet"]["configured"],
            false
        );

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 2);
    }

    #[tokio::test]
    async fn content_publish_accepts_carrier_announced_availability() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "carrier_announced",
                    "provider": "carrier-availability",
                    "policy": "network_default",
                    "replicas": 1,
                    "transport": "carrier-gossip",
                    "topic": "elastos://carrier/content/test/availability",
                    "peer_selection": {
                        "mode": "carrier_topic",
                        "topic": "elastos://carrier/content/test/availability",
                        "live_multi_peer_proof": true
                    },
                    "quota": {
                        "policy": "carrier_policy",
                        "max_replicas": 1
                    },
                    "repair_worker": {
                        "scheduled": false,
                        "status": "carrier_announced"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBDYXJyaWVyCg=="}],
                "object_did": "did:key:z6Mkobject",
                "publisher_did": "did:key:z6Mkpublisher",
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["availability"]["status"],
            "carrier_announced"
        );
        assert_eq!(
            response["data"]["availability"]["provider"],
            "carrier-availability"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "carrier_announced"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["provider"],
            "carrier-availability"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["policy"],
            "network_default"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["peer_selection"]["topic"],
            "elastos://carrier/content/test/availability"
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["quota"]["policy"],
            "carrier_policy"
        );

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "ensure");
        assert_eq!(requests[0]["local"]["replicas"], 1);
        assert_eq!(requests[0]["policy"], "network_default");
    }

    #[tokio::test]
    async fn content_publish_rejects_unproven_multi_peer_availability_claim() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "thin-availability-provider",
                    "policy": "network_default",
                    "replicas": 2
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBVbnByb3Zlbgo="}],
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["availability"]["status"], "repair_needed");
        assert_eq!(
            response["data"]["availability"]["provider"],
            "thin-availability-provider"
        );
        assert!(response["data"]["availability"]["reason"]
            .as_str()
            .unwrap()
            .contains("peer_selection"));
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "repair_needed"
        );
    }

    #[tokio::test]
    async fn content_publish_enforces_availability_requirements() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "elacity-supernode",
                    "policy": "smartweb_default",
                    "replicas": 2,
                    "peer_selection": {
                        "mode": "carrier_topic",
                        "live_multi_peer_proof": true
                    },
                    "quota": {
                        "policy": "operator_default",
                        "max_replicas": 2
                    },
                    "repair_worker": {
                        "scheduled": false,
                        "status": "healthy"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBSZXF1aXJlbWVudAo="}],
                "pin": true,
                "availability_requirements": {
                    "min_replicas": 3,
                    "max_replicas": 3,
                    "require_live_multi_peer_proof": true
                }
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["availability"]["status"], "repair_needed");
        assert!(response["data"]["availability"]["reason"]
            .as_str()
            .unwrap()
            .contains("below required 3"));
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "repair_needed"
        );

        let requests = availability.requests.lock().await;
        assert_eq!(requests[0]["requirements"]["min_replicas"], 3);
        assert_eq!(requests[0]["requirements"]["max_replicas"], 3);
        assert_eq!(
            requests[0]["requirements"]["require_live_multi_peer_proof"],
            true
        );
    }

    #[tokio::test]
    async fn content_publish_requires_peer_selection_policy_metadata() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "policyless-availability",
                    "policy": "network_default",
                    "replicas": 1,
                    "peer_selection": {
                        "live_multi_peer_proof": false
                    },
                    "quota": {
                        "policy": "operator_default",
                        "max_replicas": 1
                    },
                    "repair_worker": {
                        "scheduled": false,
                        "status": "healthy"
                    }
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBQb2xpY3kK"}],
                "pin": true,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["availability"]["status"], "repair_needed");
        assert!(response["data"]["availability"]["reason"]
            .as_str()
            .unwrap()
            .contains("peer_selection requires mode or strategy"));
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "repair_needed"
        );
    }

    #[tokio::test]
    async fn content_publish_directory_injects_object_manifest() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "document",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "object_did": "did:key:z6Mkobject",
                "publisher_did": "did:key:z6Mkpublisher",
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        let directories = ipfs.added_directories.lock().await;
        let manifest_entry = directories[0]
            .iter()
            .find(|entry| entry["path"].as_str() == Some(OBJECT_MANIFEST_PATH))
            .expect("object manifest should be injected");
        let manifest_bytes = base64::engine::general_purpose::STANDARD
            .decode(manifest_entry["data"].as_str().unwrap())
            .unwrap();
        let manifest: ContentObjectManifest = serde_json::from_slice(&manifest_bytes).unwrap();

        assert_eq!(manifest.schema, OBJECT_MANIFEST_SCHEMA);
        assert_eq!(manifest.kind, "document");
        assert_eq!(manifest.object_did.as_deref(), Some("did:key:z6Mkobject"));
        assert_eq!(
            manifest.publisher_did.as_deref(),
            Some("did:key:z6Mkpublisher")
        );
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "index.md");
        assert!(manifest.content_digest.starts_with("sha256:"));
    }

    fn sealed_object_value() -> Value {
        json!({
            "schema": "elastos.sealed.object/v1",
            "payload_cid": TEST_CID,
            "rights_policy_cid": TEST_CID,
            "availability_receipt_cid": TEST_CID,
            "key_envelope": {
                "scheme": "elastos-pq-hybrid-threshold-v0",
                "kid": "kid:test",
                "wrapped_cek": "wrapped",
                "policy_hash": "sha256:test",
                "algorithms": {
                    "cipher": "aes-256-gcm",
                    "signature": ["ed25519", "ml-dsa-65"],
                    "kem": ["x25519", "ml-kem-768"],
                    "share_scheme": "shamir-t-of-n"
                }
            },
            "viewer": {
                "required_interface": "elastos.viewer/document@1"
            }
        })
    }

    fn sealed_object_data() -> String {
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&sealed_object_value()).unwrap())
    }

    fn sealed_object_data_from(value: &Value) -> String {
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(value).unwrap())
    }

    fn sealed_object_links() -> Vec<Value> {
        vec![
            json!({"rel": "availability.receipt", "cid": TEST_CID}),
            json!({"rel": "payload", "cid": TEST_CID}),
            json!({"rel": "provenance", "cid": TEST_CID}),
            json!({"rel": "rights.policy", "cid": TEST_CID}),
        ]
    }

    #[tokio::test]
    async fn content_publish_directory_accepts_linked_release_and_sealed_manifests() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "sealed",
                "links": sealed_object_links(),
                "files": [{"path": "sealed.json", "data": sealed_object_data()}],
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        let directories = ipfs.added_directories.lock().await;
        let manifest_entry = directories[0]
            .iter()
            .find(|entry| entry["path"].as_str() == Some(OBJECT_MANIFEST_PATH))
            .expect("object manifest should be injected");
        let manifest_bytes = base64::engine::general_purpose::STANDARD
            .decode(manifest_entry["data"].as_str().unwrap())
            .unwrap();
        let manifest: ContentObjectManifest = serde_json::from_slice(&manifest_bytes).unwrap();

        assert_eq!(manifest.kind, "sealed");
        assert_eq!(manifest.links.len(), 4);
        assert_eq!(manifest.links[0].rel, "availability.receipt");
        assert_eq!(manifest.links[0].cid, TEST_CID);
        assert_eq!(manifest.links[1].rel, "payload");
        assert_eq!(manifest.links[1].cid, TEST_CID);
        assert_eq!(manifest.links[2].rel, "provenance");
        assert_eq!(manifest.links[2].cid, TEST_CID);
        assert_eq!(manifest.links[3].rel, "rights.policy");
        assert_eq!(manifest.links[3].cid, TEST_CID);
        drop(directories);

        let release_response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "release",
                "links": [{"rel": "sealed", "cid": TEST_CID}],
                "files": [{"path": "release.json", "data": "e30="}],
            }))
            .await
            .unwrap();
        assert_eq!(release_response["status"], "ok");
    }

    #[tokio::test]
    async fn content_publish_directory_rejects_incomplete_sealed_objects() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let missing_descriptor = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "sealed",
                "links": sealed_object_links(),
                "files": [{"path": "payload.bin", "data": "c2VhbGVkCg=="}],
            }))
            .await
            .unwrap_err();
        assert!(missing_descriptor
            .to_string()
            .contains("sealed content object requires sealed.json"));

        let missing_provenance = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "sealed",
                "links": [
                    {"rel": "availability.receipt", "cid": TEST_CID},
                    {"rel": "payload", "cid": TEST_CID},
                    {"rel": "rights.policy", "cid": TEST_CID}
                ],
                "files": [{"path": "sealed.json", "data": sealed_object_data()}],
            }))
            .await
            .unwrap_err();
        assert!(missing_provenance
            .to_string()
            .contains("sealed content object requires provenance link"));

        let mut weak_envelope = sealed_object_value();
        weak_envelope["key_envelope"]["algorithms"]["cipher"] = Value::String("aes-128-gcm".into());
        let weak_cipher = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "sealed",
                "links": sealed_object_links(),
                "files": [{"path": "sealed.json", "data": sealed_object_data_from(&weak_envelope)}],
            }))
            .await
            .unwrap_err();
        assert!(weak_cipher
            .to_string()
            .contains("key_envelope.algorithms.cipher uses unsupported algorithm"));
    }

    #[tokio::test]
    async fn content_publish_directory_sorts_entries_for_stable_cids() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "share",
                "files": [
                    {"path": "z.md", "data": "eg=="},
                    {"path": "a.md", "data": "YQ=="}
                ],
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        let directories = ipfs.added_directories.lock().await;
        let paths = directories[0]
            .iter()
            .map(|entry| entry["path"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec![OBJECT_MANIFEST_PATH, "a.md", "z.md"]);
    }

    #[tokio::test]
    async fn content_publish_directory_rejects_ambiguous_object_shape() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let duplicate_path = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "share",
                "files": [
                    {"path": "index.md", "data": "YQ=="},
                    {"path": "index.md", "data": "Yg=="}
                ],
            }))
            .await
            .unwrap_err();
        assert!(duplicate_path
            .to_string()
            .contains("duplicate directory publish path"));

        let unknown_kind = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "random",
                "files": [{"path": "index.md", "data": "YQ=="}],
            }))
            .await
            .unwrap_err();
        assert!(unknown_kind
            .to_string()
            .contains("unsupported content object kind"));

        let invalid_link = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "release",
                "links": [{"rel": "Bad Rel", "cid": TEST_CID}],
                "files": [{"path": "release.json", "data": "e30="}],
            }))
            .await
            .unwrap_err();
        assert!(invalid_link.to_string().contains("content object link rel"));

        let invalid_link_cid = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "object_kind": "release",
                "links": [{"rel": "release", "cid": "not-a-cid"}],
                "files": [{"path": "release.json", "data": "e30="}],
            }))
            .await
            .unwrap_err();
        assert!(invalid_link_cid
            .to_string()
            .contains("invalid content object link cid"));
    }

    #[tokio::test]
    async fn content_unpublish_wraps_ipfs_unpin() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "unpublish",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["cid"], TEST_CID);
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "local_unpinned"
        );
        assert_eq!(
            ipfs.unpinned.lock().await.as_slice(),
            [TEST_CID.to_string()]
        );
    }

    #[tokio::test]
    async fn content_repair_pins_cid_and_records_receipt() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "repair",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["availability"]["status"], "local_pinned");
        assert_eq!(
            response["data"]["receipt"]["payload"]["policy"],
            "local_repair_pin"
        );
        assert_eq!(ipfs.pinned.lock().await.as_slice(), [TEST_CID.to_string()]);
    }

    #[tokio::test]
    async fn content_ensure_pins_cid_and_records_policy() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "ensure",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["availability"]["status"], "local_pinned");
        assert_eq!(
            response["data"]["receipt"]["payload"]["policy"],
            "local_ensure_pin"
        );
        assert_eq!(ipfs.pinned.lock().await.as_slice(), [TEST_CID.to_string()]);
    }

    #[tokio::test]
    async fn content_repair_records_repair_needed_when_pin_fails() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        *ipfs.pin_error.lock().await = Some("not available".to_string());

        let response = content
            .send_raw(&json!({
                "op": "repair",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["availability"]["status"], "repair_needed");
        assert_eq!(response["data"]["availability"]["reason"], "not available");
        assert_eq!(
            response["data"]["availability"]["repair_worker"]["scheduled"],
            true
        );
        assert_eq!(
            response["data"]["receipt"]["payload"]["status"],
            "repair_needed"
        );
    }

    #[tokio::test]
    async fn content_publish_file_wraps_ipfs_bytes_with_receipt() {
        let (_data_dir, registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let cid = publish_bytes_via_provider(
            &registry,
            "provenance.json",
            br#"{"ok":true}"#,
            Some("did:key:z6Mkobject"),
            Some("did:key:z6Mkpublisher"),
        )
        .await
        .unwrap();

        assert_eq!(cid, TEST_CID);
        assert_eq!(
            ipfs.added_files.lock().await.as_slice(),
            ["provenance.json".to_string()]
        );
        let status = content
            .send_raw(&json!({
                "op": "status",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["receipt"]["payload"]["accounting"]["source"],
            "publish_request"
        );
        assert_eq!(
            status["data"]["receipt"]["payload"]["accounting"]["files"],
            1
        );
        assert_eq!(
            status["data"]["receipt"]["payload"]["accounting"]["content_bytes"],
            br#"{"ok":true}"#.len() as u64
        );
        assert_eq!(
            status["data"]["availability"]["accounting"]["storage_quota"]["enforced"],
            false
        );
    }

    #[tokio::test]
    async fn content_fetch_wraps_ipfs_cat() {
        let (_data_dir, registry, ipfs, _content) = registry_with_content_and_ipfs().await;
        let bytes = fetch_bytes_via_provider(&registry, TEST_CID, Some("capsule.json"))
            .await
            .unwrap();

        assert_eq!(bytes, b"hello content");
        let requests = ipfs.requests.lock().await;
        let cat = requests
            .iter()
            .find(|request| request["op"] == "cat")
            .expect("content helper should fetch through ipfs cat");
        assert_eq!(cat["_runtime_invocation"]["transfer"], "stream");
        assert_eq!(
            cat["_runtime_invocation"]["stream"]["mode"],
            "runtime_stream_session"
        );
        assert_eq!(
            cat["_runtime_invocation"]["abi"]["backpressure"],
            "read_next"
        );
        assert_eq!(cat["_runtime_invocation"]["abi"]["cancel_supported"], true);
    }

    #[tokio::test]
    async fn content_fetch_propagates_range_progress_transfer_receipt() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "capsule.json",
                "range": {
                    "start": 0,
                    "end": 4
                },
                "progress": {
                    "request_id": "content-fetch:test",
                    "expected_bytes": 5
                }
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["data"],
            base64::engine::general_purpose::STANDARD.encode(b"hello")
        );
        assert_eq!(
            response["data"]["transfer"]["schema"],
            "elastos.provider.transfer/v1"
        );
        assert_eq!(response["data"]["transfer"]["source"], "content-provider");
        assert_eq!(response["data"]["transfer"]["target"], "ipfs");
        assert_eq!(response["data"]["transfer"]["op"], "cat");
        assert_eq!(
            response["data"]["transfer"]["transport"],
            "runtime-local-provider-plane"
        );
        assert_eq!(response["data"]["transfer"]["range"]["start"], 0);
        assert_eq!(response["data"]["transfer"]["range"]["end"], 4);
        assert_eq!(
            response["data"]["transfer"]["progress"]["request_id"],
            "content-fetch:test"
        );
        assert_eq!(
            response["data"]["transfer"]["progress"]["expected_bytes"],
            5
        );
    }

    #[tokio::test]
    async fn content_fetch_stream_returns_provider_stream_payload() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let response = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "capsule.json",
                "transfer": "stream",
                "range": {
                    "start": 0,
                    "end": 4
                },
                "progress": {
                    "request_id": "content-stream:test",
                    "expected_bytes": 5
                }
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert!(response["data"].get("data").is_none());
        assert_eq!(
            response["data"]["stream"]["schema"],
            "elastos.provider.stream/v1"
        );
        assert_eq!(
            decode_test_stream_payload(&response["data"]["stream"]),
            b"hello"
        );
        assert_eq!(response["data"]["transfer"]["transfer"], "stream");
        assert_eq!(
            response["data"]["transfer"]["stream"]["schema"],
            "elastos.provider.stream/v1"
        );
        assert_eq!(response["data"]["transfer"]["range"]["start"], 0);
        assert_eq!(response["data"]["transfer"]["range"]["end"], 4);
        assert_eq!(
            response["data"]["transfer"]["progress"]["request_id"],
            "content-stream:test"
        );
        assert_eq!(
            response["data"]["transfer"]["progress"]["expected_bytes"],
            5
        );
    }

    #[tokio::test]
    async fn content_fetch_uses_availability_provider_when_local_backend_misses() {
        let (_data_dir, registry, ipfs, content) = registry_with_content_and_ipfs().await;
        ipfs.missing_paths
            .lock()
            .await
            .push("remote.md".to_string());
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(b"remote content"),
                "availability": {
                    "status": "network_available",
                    "provider": "mock-availability",
                    "policy": "network_default",
                    "replicas": 2
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "remote.md",
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["data"],
            base64::engine::general_purpose::STANDARD.encode(b"remote content")
        );
        assert_eq!(
            response["data"]["availability"]["provider"],
            "mock-availability"
        );
        assert_eq!(
            response["data"]["availability"]["status"],
            "network_available"
        );

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "fetch");
        assert_eq!(requests[0]["cid"], TEST_CID);
        assert_eq!(requests[0]["uri"], format!("elastos://{TEST_CID}"));
        assert_eq!(requests[0]["path"], "remote.md");
    }

    #[tokio::test]
    async fn content_fetch_ranges_availability_provider_when_local_backend_misses() {
        let (_data_dir, registry, ipfs, content) = registry_with_content_and_ipfs().await;
        ipfs.missing_paths
            .lock()
            .await
            .push("remote.md".to_string());
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(b"remote content"),
                "availability": {
                    "status": "network_available",
                    "provider": "mock-availability",
                    "policy": "network_default",
                    "replicas": 2
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "remote.md",
                "range": {
                    "start": 7,
                    "end": 13
                },
                "progress": {
                    "request_id": "availability-fetch:test",
                    "expected_bytes": 7
                }
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["data"],
            base64::engine::general_purpose::STANDARD.encode(b"content")
        );
        assert_eq!(response["data"]["transfer"]["target"], "availability");
        assert_eq!(
            response["data"]["transfer"]["transport"],
            "runtime-local-provider-plane"
        );
        assert_eq!(response["data"]["transfer"]["range"]["start"], 7);
        assert_eq!(response["data"]["transfer"]["range"]["end"], 13);
        assert_eq!(
            response["data"]["transfer"]["progress"]["request_id"],
            "availability-fetch:test"
        );

        let requests = availability.requests.lock().await;
        assert_eq!(
            requests[0]["_runtime_invocation"]["source"],
            "content-provider"
        );
        assert_eq!(requests[0]["_runtime_invocation"]["target"], "availability");
        assert_eq!(requests[0]["_runtime_invocation"]["range"]["start"], 7);
    }

    #[tokio::test]
    async fn content_fetch_stream_ranges_availability_provider_when_local_backend_misses() {
        let (_data_dir, registry, ipfs, content) = registry_with_content_and_ipfs().await;
        ipfs.missing_paths
            .lock()
            .await
            .push("remote.md".to_string());
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(b"remote content"),
                "availability": {
                    "status": "network_available",
                    "provider": "mock-availability",
                    "policy": "network_default",
                    "replicas": 2
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let response = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "remote.md",
                "transfer": "stream",
                "range": {
                    "start": 7,
                    "end": 13
                },
                "progress": {
                    "request_id": "availability-stream:test",
                    "expected_bytes": 7
                }
            }))
            .await
            .unwrap();

        assert_eq!(response["status"], "ok");
        assert!(response["data"].get("data").is_none());
        assert_eq!(
            decode_test_stream_payload(&response["data"]["stream"]),
            b"content"
        );
        assert_eq!(
            response["data"]["availability"]["provider"],
            "mock-availability"
        );
        assert_eq!(response["data"]["transfer"]["target"], "availability");
        assert_eq!(response["data"]["transfer"]["transfer"], "stream");
        assert_eq!(
            response["data"]["transfer"]["progress"]["request_id"],
            "availability-stream:test"
        );

        let requests = availability.requests.lock().await;
        assert_eq!(
            requests[0]["_runtime_invocation"]["source"],
            "content-provider"
        );
        assert_eq!(requests[0]["_runtime_invocation"]["target"], "availability");
        assert_eq!(requests[0]["_runtime_invocation"]["transfer"], "stream");
        assert_eq!(
            requests[0]["_runtime_invocation"]["stream"]["schema"],
            "elastos.provider.stream/v1"
        );
        assert_eq!(requests[0]["_runtime_invocation"]["range"]["start"], 7);
    }

    #[tokio::test]
    async fn content_fetch_local_only_skips_availability_provider() {
        let (_data_dir, registry, ipfs, content) = registry_with_content_and_ipfs().await;
        ipfs.missing_paths
            .lock()
            .await
            .push("remote.md".to_string());
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "data": base64::engine::general_purpose::STANDARD.encode(b"remote content"),
                "availability": {
                    "status": "network_available",
                    "provider": "mock-availability"
                }
            }))),
        });
        registry.register(availability.clone()).await;

        let err = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "remote.md",
                "local_only": true,
            }))
            .await
            .expect_err("local_only fetch must fail on local backend miss");

        assert!(err.to_string().contains("content fetch"));
        assert!(availability.requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn content_prepare_data_capsule_materializes_verified_manifest_files() {
        let (_data_dir, registry, ipfs, _content) = registry_with_content_and_ipfs().await;
        let capsule_json = serde_json::json!({
            "schema": elastos_common::SCHEMA_V1,
            "version": "0.1.0",
            "name": "shared-doc",
            "role": "content",
            "type": "data",
            "entrypoint": "index.html"
        });
        let capsule_bytes = serde_json::to_vec(&capsule_json).unwrap();
        let index_bytes = b"<html>viewer</html>".to_vec();
        let markdown_bytes = b"# Hello\n".to_vec();
        let object_manifest = ContentObjectManifest {
            schema: OBJECT_MANIFEST_SCHEMA.to_string(),
            kind: "share".to_string(),
            content_digest: "sha256:test".to_string(),
            files: vec![
                object_file("capsule.json", &capsule_bytes),
                object_file("docs/readme.md", &markdown_bytes),
                object_file("index.html", &index_bytes),
            ],
            links: Vec::new(),
            object_did: None,
            publisher_did: None,
        };
        let object_manifest_bytes = serde_json::to_vec(&object_manifest).unwrap();

        {
            let mut cat_files = ipfs.cat_files.lock().await;
            cat_files.insert("capsule.json".to_string(), capsule_bytes);
            cat_files.insert(OBJECT_MANIFEST_PATH.to_string(), object_manifest_bytes);
            cat_files.insert("index.html".to_string(), index_bytes.clone());
            cat_files.insert("docs/readme.md".to_string(), markdown_bytes.clone());
        }

        let capsule_dir = prepare_capsule_from_content_provider(&registry, TEST_CID)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(capsule_dir.join("index.html")).unwrap(),
            index_bytes
        );
        assert_eq!(
            std::fs::read(capsule_dir.join("docs/readme.md")).unwrap(),
            markdown_bytes
        );
        assert!(capsule_dir.join(OBJECT_MANIFEST_PATH).is_file());
        std::fs::remove_dir_all(capsule_dir).unwrap();
    }

    #[tokio::test]
    async fn content_prepare_data_capsule_rejects_object_hash_mismatch() {
        let (_data_dir, registry, ipfs, _content) = registry_with_content_and_ipfs().await;
        let capsule_json = serde_json::json!({
            "schema": elastos_common::SCHEMA_V1,
            "version": "0.1.0",
            "name": "shared-doc",
            "role": "content",
            "type": "data",
            "entrypoint": "index.html"
        });
        let capsule_bytes = serde_json::to_vec(&capsule_json).unwrap();
        let original_index = b"<html>viewer</html>".to_vec();
        let tampered_index = b"<html>viewed</html>".to_vec();
        let object_manifest = ContentObjectManifest {
            schema: OBJECT_MANIFEST_SCHEMA.to_string(),
            kind: "share".to_string(),
            content_digest: "sha256:test".to_string(),
            files: vec![
                object_file("capsule.json", &capsule_bytes),
                object_file("index.html", &original_index),
            ],
            links: Vec::new(),
            object_did: None,
            publisher_did: None,
        };
        let object_manifest_bytes = serde_json::to_vec(&object_manifest).unwrap();

        {
            let mut cat_files = ipfs.cat_files.lock().await;
            cat_files.insert("capsule.json".to_string(), capsule_bytes);
            cat_files.insert(OBJECT_MANIFEST_PATH.to_string(), object_manifest_bytes);
            cat_files.insert("index.html".to_string(), tampered_index);
        }

        let err = prepare_capsule_from_content_provider(&registry, TEST_CID)
            .await
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("content object file hash mismatch"));
    }

    #[tokio::test]
    async fn content_prepare_capsule_rejects_release_object_as_not_launchable() {
        let (_data_dir, registry, ipfs, _content) = registry_with_content_and_ipfs().await;
        let release_bytes = br#"{"payload":{},"signature":"00","signer_did":"did:key:z6Mk"}"#;
        let object_manifest = ContentObjectManifest {
            schema: OBJECT_MANIFEST_SCHEMA.to_string(),
            kind: "release".to_string(),
            content_digest: "sha256:test".to_string(),
            files: vec![object_file("release.json", release_bytes)],
            links: Vec::new(),
            object_did: Some("elastos://release/stable/0.2.0".to_string()),
            publisher_did: Some("did:key:z6Mkpublisher".to_string()),
        };
        let object_manifest_bytes = serde_json::to_vec(&object_manifest).unwrap();

        {
            let mut cat_files = ipfs.cat_files.lock().await;
            cat_files.insert(OBJECT_MANIFEST_PATH.to_string(), object_manifest_bytes);
        }
        ipfs.missing_paths
            .lock()
            .await
            .push("capsule.json".to_string());

        let err = prepare_capsule_from_content_provider(&registry, TEST_CID)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("kind 'release'"));
        assert!(err.to_string().contains("not a launchable capsule"));
    }

    #[tokio::test]
    async fn content_fetch_rejects_invalid_cid_and_path() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let invalid_cid = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": "not-a-cid",
            }))
            .await
            .unwrap();
        assert_eq!(invalid_cid["status"], "error");
        assert_eq!(invalid_cid["code"], "invalid_cid");

        let invalid_path = content
            .send_raw(&json!({
                "op": "fetch",
                "cid": TEST_CID,
                "path": "../secret",
            }))
            .await
            .unwrap();
        assert_eq!(invalid_path["status"], "error");
        assert_eq!(invalid_path["code"], "invalid_path");
    }

    #[tokio::test]
    async fn content_status_rejects_invalid_cid() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let invalid_cid = content
            .send_raw(&json!({
                "op": "status",
                "cid": "not-a-cid",
            }))
            .await
            .unwrap();

        assert_eq!(invalid_cid["status"], "error");
        assert_eq!(invalid_cid["code"], "invalid_cid");
    }

    fn object_file(path: &str, bytes: &[u8]) -> ContentObjectFile {
        ContentObjectFile {
            path: path.to_string(),
            sha256: format!("{:x}", sha2::Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }

    #[tokio::test]
    async fn content_status_reads_latest_availability_receipt() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
                "object_did": "did:key:z6Mkobject",
                "publisher_did": "did:key:z6Mkpublisher",
            }))
            .await
            .unwrap();
        content
            .send_raw(&json!({
                "op": "unpublish",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();

        let status = content
            .send_raw(&json!({
                "op": "status",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();

        assert_eq!(status["status"], "ok");
        assert_eq!(status["data"]["cid"], TEST_CID);
        assert_eq!(status["data"]["availability"]["status"], "local_unpinned");
        assert_eq!(status["data"]["availability"]["policy"], "local_unpublish");
        assert_eq!(
            status["data"]["availability"]["peer_selection"]["mode"],
            "single_local"
        );
        assert_eq!(
            status["data"]["availability"]["quota"]["policy"],
            "not_enforced"
        );
        assert_eq!(
            status["data"]["availability"]["repair_worker"]["scheduled"],
            false
        );
        assert_eq!(
            status["data"]["availability"]["abuse_controls"]["policy"],
            "local_content_backend"
        );
        assert_eq!(
            status["data"]["availability"]["repair_task"]["status"],
            "retired"
        );
        assert_eq!(
            status["data"]["receipt"]["payload"]["schema"],
            AVAILABILITY_RECEIPT_SCHEMA
        );
    }

    #[tokio::test]
    async fn content_status_without_cid_returns_availability_dashboard() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();

        assert_eq!(status["status"], "ok");
        assert_eq!(status["data"]["schema"], AVAILABILITY_DASHBOARD_SCHEMA);
        assert_eq!(status["data"]["objects"]["tracked"], 1);
        assert_eq!(status["data"]["objects"]["by_status"]["local_pinned"], 1);
        assert_eq!(status["data"]["objects"]["by_provider"]["ipfs-provider"], 1);
        assert_eq!(status["data"]["quota"]["by_status"]["not_enforced"], 1);
        assert_eq!(status["data"]["quota"]["enforced"], 0);
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["schema"],
            CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["status"],
            "federated_quota_ledger_not_configured"
        );
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["remote"]["signed_admission_receipts"],
            0
        );
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["federation"]
                ["signed_admission_receipt_exchange"],
            false
        );
        assert_eq!(status["data"]["proofs"]["live_multi_peer"], 0);
        assert_eq!(status["data"]["proofs"]["remote_receipts"], 0);
        assert_eq!(
            status["data"]["proofs"]["peer_attestation_exchange_policy"]["schema"],
            CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["proofs"]["peer_attestation_exchange_policy"]["status"],
            "attestation_exchange_not_configured"
        );
        assert_eq!(
            status["data"]["proofs"]["peer_attestation_exchange_policy"]["attestation_exchange"]
                ["configured"],
            false
        );
        assert_eq!(status["data"]["accounting"]["accounted_objects"], 1);
        assert_eq!(status["data"]["accounting"]["accounted_files"], 1);
        assert_eq!(status["data"]["accounting"]["content_bytes"], 7);
        assert_eq!(status["data"]["accounting"]["replica_bytes_estimate"], 7);
        assert_eq!(
            status["data"]["accounting"]["by_source"]["publish_request"],
            1
        );
        assert_eq!(
            status["data"]["accounting"]["storage_quota_policy"],
            "principal_ledger"
        );
        assert_eq!(
            status["data"]["accounting"]["ledger"]["schema"],
            CONTENT_STORAGE_ACCOUNTING_LEDGER_SCHEMA
        );
        assert_eq!(status["data"]["accounting"]["ledger"]["durable"], true);
        assert_eq!(status["data"]["accounting"]["ledger"]["tracked_objects"], 1);
        assert_eq!(status["data"]["accounting"]["ledger"]["active_objects"], 1);
        assert_eq!(
            status["data"]["accounting"]["ledger"]["tracked_principals"],
            1
        );
        assert_eq!(status["data"]["accounting"]["ledger"]["content_bytes"], 7);
        assert_eq!(
            status["data"]["accounting"]["ledger"]["replica_bytes_estimate"],
            7
        );
        assert_eq!(
            status["data"]["accounting"]["ledger"]["market_policy"]["settlement"],
            "not_configured"
        );
        assert_eq!(
            status["data"]["accounting"]["ledger"]["market_policy"]["admission_policy"]["schema"],
            CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["accounting"]["ledger"]["market_policy"]["settlement_policy"]["schema"],
            CONTENT_STORAGE_SETTLEMENT_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["storage_settlement_policy"]["schema"],
            CONTENT_STORAGE_SETTLEMENT_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["storage_settlement_policy"]["status"],
            "settlement_not_configured"
        );
        assert_eq!(
            status["data"]["storage_settlement_policy"]["production_federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["storage_market_admission_policy"]["schema"],
            CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["storage_market_admission_policy"]["status"],
            "production_storage_market_admission_not_configured"
        );
        assert_eq!(
            status["data"]["storage_market_admission_policy"]["production_market"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["storage_market_admission_policy"]["current_admission"]
                ["signed_admission_receipts"],
            0
        );
        let principals = status["data"]["accounting"]["ledger"]["by_principal"]
            .as_object()
            .expect("ledger should group by principal");
        assert_eq!(principals.len(), 1);
        let principal = principals.values().next().unwrap();
        assert_eq!(principal["active_objects"], 1);
        assert_eq!(principal["content_bytes"], 7);
        assert_eq!(status["data"]["abuse_controls"]["enforced"], 0);
        assert_eq!(status["data"]["abuse_controls"]["throttled"], 0);
        assert_eq!(
            status["data"]["abuse_controls"]["by_policy"]["local_content_backend"],
            1
        );
        assert_eq!(
            status["data"]["network_abuse_policy"]["schema"],
            CONTENT_NETWORK_ABUSE_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["network_abuse_policy"]["local_guardrails"]
                ["provider_invocation_required"],
            true
        );
        assert_eq!(
            status["data"]["network_abuse_policy"]["network_federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["schema"],
            CONTENT_OPERATOR_DASHBOARD_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["storage_pressure"]["status"],
            "accounting_observed"
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["storage_pressure"]["content_bytes"],
            7
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["storage_pressure"]["settlement_policy"]["status"],
            "settlement_not_configured"
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["storage_pressure"]["market_admission_policy"]
                ["schema"],
            CONTENT_STORAGE_MARKET_ADMISSION_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["storage_pressure"]["quota_ledger_policy"]
                ["schema"],
            CONTENT_FEDERATED_QUOTA_LEDGER_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["storage_pressure"]
                ["top_principals_by_content_bytes"][0]["content_bytes"],
            7
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["fleet_history"]["tracked_tasks"],
            1
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["fleet_history"]["recent"][0]["status"],
            "local_only"
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["fleet_history"]["external_repair_fleet_policy"]
                ["schema"],
            EXTERNAL_REPAIR_FLEET_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["production_federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["proof_summary"]
                ["peer_attestation_exchange_policy"]["schema"],
            CARRIER_PEER_ATTESTATION_EXCHANGE_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["proof_summary"]
                ["peer_attestation_exchange_policy"]["status"],
            "attestation_exchange_not_configured"
        );
        assert_eq!(status["data"]["repair"]["tracked_tasks"], 1);
        assert_eq!(status["data"]["repair"]["by_status"]["local_only"], 1);
        assert_eq!(
            status["data"]["scheduler"]["manual_trigger"],
            "elastos content repair-worker"
        );
        assert_eq!(
            status["data"]["scheduler"]["provider_invocation_required"],
            true
        );
        assert_eq!(
            status["data"]["repair_fleet"]["schema"],
            REPAIR_FLEET_SCHEMA
        );
        assert_eq!(
            status["data"]["repair_fleet"]["policy"],
            "single_runtime_provider_repair_fleet"
        );
        assert_eq!(
            status["data"]["repair_fleet"]["coordinator"]["provider"],
            "content-provider"
        );
        assert_eq!(
            status["data"]["repair_fleet"]["workers"][0]["runtime_invocation_required"],
            true
        );
        assert_eq!(
            status["data"]["repair_fleet"]["task_pressure"]["tracked"],
            1
        );
        assert_eq!(
            status["data"]["repair_fleet"]["production_federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["external_repair_fleet_policy"]["schema"],
            EXTERNAL_REPAIR_FLEET_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["external_repair_fleet_policy"]["external_fleet"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["schema"],
            CONTENT_FEDERATED_OPERATOR_ALERTING_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["status"],
            "provider_local_dashboard_only"
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["local_signals"]
                ["storage_pressure_status"],
            "accounting_observed"
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["local_signals"]["content_bytes"],
            7
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["federated_operator_alerting_policy"]["schema"],
            CONTENT_FEDERATED_OPERATOR_ALERTING_POLICY_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_dashboard"]["federated_operator_alerting_policy"]
                ["local_dashboard"]["schema"],
            CONTENT_OPERATOR_DASHBOARD_SCHEMA
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["operator_alert_sink"]
                ["configured"],
            false
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federation"]["alert_delivery"],
            false
        );
    }

    #[tokio::test]
    async fn content_status_can_emit_operator_alert_receipt_without_sink() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();

        let status = content
            .send_raw(&json!({
                "op": "status",
                "emit_operator_alert": true,
            }))
            .await
            .unwrap();

        assert_eq!(
            status["data"]["operator_alert_delivery"]["schema"],
            CONTENT_OPERATOR_ALERT_RECEIPT_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["delivery"]["status"],
            "not_configured"
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["alert"]["schema"],
            CONTENT_OPERATOR_ALERT_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["alert"]["local_signals"]
                ["storage_pressure_status"],
            "accounting_observed"
        );
        let outbox = std::fs::read_to_string(content.operator_alert_receipts_path()).unwrap();
        assert!(outbox.contains(CONTENT_OPERATOR_ALERT_RECEIPT_SCHEMA));
        assert!(outbox.contains(CONTENT_OPERATOR_ALERT_SCHEMA));
    }

    #[tokio::test]
    async fn content_status_delivers_operator_alert_to_configured_loopback_sink() {
        let (url, handle) = spawn_operator_alert_sink();
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_alert_config(Some(json!({
                "url": url,
                "authorization": "Bearer operator-alert-test",
                "timeout_secs": 5,
            })))
            .await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();

        let status = content
            .send_raw(&json!({
                "op": "status",
                "emit_operator_alert": true,
            }))
            .await
            .unwrap();

        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["status"],
            "provider_local_alert_sink_configured"
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["operator_alert_sink"]
                ["configured"],
            true
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["operator_alert_sink"]
                ["credential_exposed"],
            false
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federation"]["alert_delivery"],
            true
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federation"]["configured"],
            false
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["delivery"]["status"],
            "delivered"
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["delivery"]["http_status"],
            204
        );
        let request = handle.join().unwrap();
        assert!(request.starts_with("POST /content-alerts HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer operator-alert-test")));
        assert!(request.contains(CONTENT_OPERATOR_ALERT_SCHEMA));
        assert!(!request
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("operator-alert-test"));
    }

    #[tokio::test]
    async fn content_status_exchanges_operator_alert_with_configured_federated_endpoint() {
        let (url, handle) = spawn_federated_operator_alert_exchange(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "operator-alert-exchange:test",
            "receipt_id": "operator-alert-receipt:123",
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_federated_alert_exchange_config(Some(json!({
                "url": url,
                "authorization": "Bearer federated-alert-test",
                "timeout_secs": 5,
            })))
            .await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
            }))
            .await
            .unwrap();

        let status = content
            .send_raw(&json!({
                "op": "status",
                "emit_operator_alert": true,
            }))
            .await
            .unwrap();

        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["status"],
            "federated_alert_exchange_configured"
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federated_alert_exchange"]
                ["configured"],
            true
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federated_alert_exchange"]
                ["credential_exposed"],
            false
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federation"]["configured"],
            true
        );
        assert_eq!(
            status["data"]["federated_operator_alerting_policy"]["federation"]
                ["fleet_alert_exchange"],
            true
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["delivery"]["status"],
            "not_configured"
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["federated_exchange"]["schema"],
            CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_RECEIPT_SCHEMA
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["federated_exchange"]["status"],
            "accepted"
        );
        assert_eq!(
            status["data"]["operator_alert_delivery"]["federated_exchange"]["remote_exchange_id"],
            "operator-alert-exchange:test"
        );
        let request = handle.join().unwrap();
        assert!(request.starts_with("POST /alerts/exchange HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer federated-alert-test")));
        assert!(request.contains(CONTENT_FEDERATED_OPERATOR_ALERT_EXCHANGE_REQUEST_SCHEMA));
        assert!(request.contains(CONTENT_OPERATOR_ALERT_SCHEMA));
        assert!(!request
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("federated-alert-test"));
    }

    #[tokio::test]
    async fn content_storage_accounting_ledger_is_durable_per_principal() {
        let (data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
                "object_did": "did:key:z6Mkobject",
                "publisher_did": "did:key:z6Mkpublisher",
            }))
            .await
            .unwrap();

        let restarted_content =
            ContentProvider::new(data_dir.path().to_path_buf(), Arc::downgrade(&registry));
        let status = restarted_content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        let principal =
            &status["data"]["accounting"]["ledger"]["by_principal"]["did:key:z6Mkpublisher"];
        assert_eq!(principal["tracked_objects"], 1);
        assert_eq!(principal["active_objects"], 1);
        assert_eq!(principal["content_bytes"], 7);
        assert_eq!(principal["replica_bytes_estimate"], 7);

        content
            .send_raw(&json!({
                "op": "unpublish",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();
        let status = restarted_content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        let ledger = &status["data"]["accounting"]["ledger"];
        assert_eq!(ledger["tracked_objects"], 1);
        assert_eq!(ledger["active_objects"], 0);
        let principal = &ledger["by_principal"]["did:key:z6Mkpublisher"];
        assert_eq!(principal["tracked_objects"], 1);
        assert_eq!(principal["active_objects"], 0);
        assert_eq!(principal["by_status"]["local_unpinned"], 1);

        let object_status = restarted_content
            .send_raw(&json!({
                "op": "status",
                "cid": TEST_CID,
            }))
            .await
            .unwrap();
        assert_eq!(
            object_status["data"]["availability"]["storage_accounting"]["principal_did"],
            "did:key:z6Mkpublisher"
        );
    }

    #[tokio::test]
    async fn content_publish_enforces_principal_storage_quota() {
        let (_data_dir, _registry, ipfs, content) = registry_with_content_and_ipfs().await;
        let rejected = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
                "publisher_did": "did:key:z6Mkpublisher",
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 6
                }
            }))
            .await
            .unwrap();

        assert_eq!(rejected["status"], "error");
        assert_eq!(rejected["code"], "storage_quota_exceeded");
        assert_eq!(*ipfs.add_count.lock().await, 0);

        let accepted = content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
                "publisher_did": "did:key:z6Mkpublisher",
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 7
                }
            }))
            .await
            .unwrap();

        assert_eq!(accepted["status"], "ok");
        assert_eq!(
            accepted["data"]["receipt"]["payload"]["accounting"]["storage_quota"]["policy"],
            "principal_storage_quota"
        );
        assert_eq!(
            accepted["data"]["receipt"]["payload"]["accounting"]["storage_quota"]["status"],
            "within_quota"
        );

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(status["data"]["accounting"]["storage_quota_enforced"], 1);
        assert_eq!(status["data"]["accounting"]["ledger"]["quota_enforced"], 1);
        assert_eq!(
            status["data"]["accounting"]["ledger"]["by_principal"]["did:key:z6Mkpublisher"]
                ["quota_enforced"],
            1
        );
    }

    #[tokio::test]
    async fn content_admission_accepts_within_principal_quota() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
                "publisher_did": "did:key:z6Mkpublisher",
            }))
            .await
            .unwrap();

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], true);
        assert_eq!(
            admission["data"]["admission"]["quota"]["status"],
            "within_quota"
        );
        assert_eq!(
            admission["data"]["admission"]["quota"]["active_content_bytes"],
            7
        );
        assert_eq!(
            admission["data"]["admission"]["quota"]["projected_content_bytes"],
            10
        );
        let signer_did = admission["data"]["receipt"]["signer_did"]
            .as_str()
            .unwrap()
            .to_string();
        let signed_receipt = serde_json::to_vec(&admission["data"]["receipt"]).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_receipt,
            CONTENT_ADMISSION_DOMAIN,
            &[signer_did],
        )
        .unwrap();
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );
    }

    #[tokio::test]
    async fn content_admission_rejects_quota_exceeded() {
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        content
            .send_raw(&json!({
                "op": "publish",
                "kind": "directory",
                "files": [{"path": "index.md", "data": "IyBUZXN0Cg=="}],
                "pin": true,
                "publisher_did": "did:key:z6Mkpublisher",
            }))
            .await
            .unwrap();

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 4,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], false);
        assert_eq!(admission["data"]["admission"]["status"], "rejected");
        assert_eq!(
            admission["data"]["admission"]["quota"]["status"],
            "quota_exceeded"
        );
        assert_eq!(
            admission["data"]["admission"]["quota"]["projected_content_bytes"],
            11
        );
        let signer_did = admission["data"]["receipt"]["signer_did"]
            .as_str()
            .unwrap()
            .to_string();
        let signed_receipt = serde_json::to_vec(&admission["data"]["receipt"]).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_receipt,
            CONTENT_ADMISSION_DOMAIN,
            &[signer_did],
        )
        .unwrap();
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );
    }

    #[tokio::test]
    async fn content_admission_records_configured_federated_quota_ledger_acceptance() {
        let receipt = signed_federated_quota_ledger_exchange_receipt(true, None);
        let (url, handle) = spawn_federated_quota_ledger_exchange_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "quota-exchange:test",
            "receipt_id": "quota-receipt:accepted",
            "receipt": receipt,
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_quota_ledger_exchange_config(Some(json!({
                "url": url,
                "authorization": "Bearer quota-test",
                "timeout_secs": 5,
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], true);
        assert_eq!(
            admission["data"]["admission"]["federated_quota_ledger_exchange"]["schema"],
            CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_RECEIPT_SCHEMA
        );
        assert_eq!(
            admission["data"]["admission"]["federated_quota_ledger_exchange"]["signed_receipt"]
                ["verified"],
            true
        );
        assert_eq!(
            admission["data"]["admission"]["quota"]["federated_quota_ledger_policy"]["status"],
            "federated_quota_ledger_accepted"
        );
        assert_eq!(
            admission["data"]["admission"]["quota"]["federated_quota_ledger_policy"]["federation"]
                ["configured"],
            true
        );
        let signer_did = admission["data"]["receipt"]["signer_did"]
            .as_str()
            .unwrap()
            .to_string();
        let signed_receipt = serde_json::to_vec(&admission["data"]["receipt"]).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_receipt,
            CONTENT_ADMISSION_DOMAIN,
            &[signer_did],
        )
        .unwrap();
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["federation"]["configured"],
            true
        );
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["federation"]["exchange_client"]
                ["authorization_configured"],
            true
        );
        assert!(!status.to_string().contains("quota-test"));

        let request = handle.join().unwrap();
        assert!(request.starts_with("POST /quota/exchange HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer quota-test")));
        assert!(request.contains(CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA));
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
        assert!(!body.contains("quota-test"));
        let signed_request: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            signed_request["payload"]["schema"],
            CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA
        );
        assert_eq!(
            signed_request["payload"]["authority"]["credential_exposed"],
            false
        );
        let request_signer = signed_request["signer_did"].as_str().unwrap().to_string();
        let signed_request_bytes = serde_json::to_vec(&signed_request).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_request_bytes,
            CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_DOMAIN,
            &[request_signer],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn content_admission_accepts_configured_federated_quota_ledger_quorum() {
        let receipt_a = signed_federated_quota_ledger_exchange_receipt(true, None);
        let (url_a, handle_a) = spawn_federated_quota_ledger_exchange_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "quota-exchange:a",
            "receipt_id": "quota-receipt:a",
            "receipt": receipt_a,
        }));
        let receipt_b = signed_federated_quota_ledger_exchange_receipt(true, None);
        let (url_b, handle_b) = spawn_federated_quota_ledger_exchange_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "quota-exchange:b",
            "receipt_id": "quota-receipt:b",
            "receipt": receipt_b,
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_quota_ledger_exchange_config(Some(json!({
                "quorum": 2,
                "endpoints": [
                    {
                        "id": "ledger-a",
                        "url": url_a,
                        "authorization": "Bearer quota-a",
                        "timeout_secs": 5
                    },
                    {
                        "id": "ledger-b",
                        "url": url_b,
                        "authorization": "Bearer quota-b",
                        "timeout_secs": 5
                    }
                ]
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        let exchange = &admission["data"]["admission"]["federated_quota_ledger_exchange"];
        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], true);
        assert_eq!(exchange["accepted"], true);
        assert_eq!(exchange["quorum"]["required"], 2);
        assert_eq!(exchange["quorum"]["endpoint_count"], 2);
        assert_eq!(exchange["quorum"]["accepted"], 2);
        assert_eq!(exchange["signed_receipt"]["verified"], true);
        assert_eq!(exchange["exchange"]["multi_endpoint"], true);
        assert_eq!(exchange["exchange"]["endpoint_count"], 2);
        assert_eq!(
            admission["data"]["admission"]["quota"]["federated_quota_ledger_policy"]["status"],
            "federated_quota_ledger_accepted"
        );

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["federation"]["exchange_client"]
                ["endpoint_count"],
            2
        );
        assert_eq!(
            status["data"]["federated_quota_ledger_policy"]["federation"]["exchange_client"]
                ["quorum_required"],
            2
        );
        assert!(!status.to_string().contains("quota-a"));
        assert!(!status.to_string().contains("quota-b"));

        let request_a = handle_a.join().unwrap();
        let request_b = handle_b.join().unwrap();
        assert!(request_a
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer quota-a")));
        assert!(request_b
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer quota-b")));
        assert!(!request_a
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("quota-a"));
        assert!(!request_b
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("quota-b"));
    }

    #[tokio::test]
    async fn content_admission_rejects_when_configured_federated_quota_ledger_rejects() {
        let receipt =
            signed_federated_quota_ledger_exchange_receipt(false, Some("ledger exhausted"));
        let (url, handle) = spawn_federated_quota_ledger_exchange_endpoint(json!({
            "accepted": false,
            "status": "rejected",
            "reason": "ledger exhausted",
            "receipt": receipt,
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_quota_ledger_exchange_config(Some(json!({
                "url": url,
                "authorization": "Bearer quota-test",
                "timeout_secs": 5,
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], false);
        assert_eq!(admission["data"]["admission"]["status"], "rejected");
        assert!(admission["data"]["admission"]["reason"]
            .as_str()
            .unwrap()
            .contains("ledger exhausted"));
        assert_eq!(
            admission["data"]["admission"]["federated_quota_ledger_exchange"]["status"],
            "rejected"
        );
        assert_eq!(
            admission["data"]["admission"]["federated_quota_ledger_exchange"]["signed_receipt"]
                ["verified"],
            true
        );
        assert_eq!(
            admission["data"]["admission"]["quota"]["federated_quota_ledger_policy"]["status"],
            "federated_quota_ledger_rejected"
        );
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );

        let request = handle.join().unwrap();
        assert!(request.contains(CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA));
    }

    #[tokio::test]
    async fn content_admission_rejects_configured_federated_quota_ledger_quorum_failure() {
        let accepted_receipt = signed_federated_quota_ledger_exchange_receipt(true, None);
        let (accepted_url, accepted_handle) =
            spawn_federated_quota_ledger_exchange_endpoint(json!({
                "accepted": true,
                "status": "accepted",
                "receipt": accepted_receipt,
            }));
        let rejected_receipt =
            signed_federated_quota_ledger_exchange_receipt(false, Some("ledger exhausted"));
        let (rejected_url, rejected_handle) =
            spawn_federated_quota_ledger_exchange_endpoint(json!({
                "accepted": false,
                "status": "rejected",
                "reason": "ledger exhausted",
                "receipt": rejected_receipt,
            }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_quota_ledger_exchange_config(Some(json!({
                "quorum": 2,
                "endpoints": [
                    {"id": "ledger-a", "url": accepted_url, "timeout_secs": 5},
                    {"id": "ledger-b", "url": rejected_url, "timeout_secs": 5}
                ]
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        let exchange = &admission["data"]["admission"]["federated_quota_ledger_exchange"];
        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], false);
        assert_eq!(admission["data"]["admission"]["status"], "rejected");
        assert!(admission["data"]["admission"]["reason"]
            .as_str()
            .unwrap()
            .contains("ledger exhausted"));
        assert_eq!(exchange["accepted"], false);
        assert_eq!(exchange["quorum"]["required"], 2);
        assert_eq!(exchange["quorum"]["accepted"], 1);
        assert_eq!(exchange["quorum"]["rejected"], 1);
        assert_eq!(exchange["signed_receipt"]["verified"], true);
        assert_eq!(
            admission["data"]["admission"]["quota"]["federated_quota_ledger_policy"]["status"],
            "federated_quota_ledger_rejected"
        );

        let accepted_request = accepted_handle.join().unwrap();
        let rejected_request = rejected_handle.join().unwrap();
        assert!(accepted_request.contains(CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA));
        assert!(rejected_request.contains(CONTENT_FEDERATED_QUOTA_LEDGER_EXCHANGE_REQUEST_SCHEMA));
    }

    #[tokio::test]
    async fn content_admission_records_configured_federated_abuse_control_acceptance() {
        let receipt = signed_federated_abuse_control_exchange_receipt(true, None);
        let (url, handle) = spawn_federated_abuse_control_exchange_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "abuse-control-exchange:test",
            "receipt_id": "abuse-control-receipt:accepted",
            "receipt": receipt,
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_abuse_control_exchange_config(Some(json!({
                "url": url,
                "authorization": "Bearer abuse-test",
                "timeout_secs": 5,
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], true);
        assert_eq!(
            admission["data"]["admission"]["federated_abuse_control_exchange"]["schema"],
            CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_RECEIPT_SCHEMA
        );
        assert_eq!(
            admission["data"]["admission"]["federated_abuse_control_exchange"]["signed_receipt"]
                ["verified"],
            true
        );
        assert_eq!(
            admission["data"]["admission"]["federated_abuse_control_exchange"]["signed_receipt"]
                ["abuse_ledger_id"],
            "abuse-ledger:test"
        );
        let signer_did = admission["data"]["receipt"]["signer_did"]
            .as_str()
            .unwrap()
            .to_string();
        let signed_receipt = serde_json::to_vec(&admission["data"]["receipt"]).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_receipt,
            CONTENT_ADMISSION_DOMAIN,
            &[signer_did],
        )
        .unwrap();
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["network_abuse_policy"]["status"],
            "configured_federated_abuse_control_exchange"
        );
        assert_eq!(
            status["data"]["network_abuse_policy"]["network_federation"]["configured"],
            true
        );
        assert_eq!(
            status["data"]["network_abuse_policy"]["network_federation"]["exchange_client"]
                ["authorization_configured"],
            true
        );
        assert!(!status.to_string().contains("abuse-test"));

        let request = handle.join().unwrap();
        assert!(request.starts_with("POST /abuse/exchange HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer abuse-test")));
        assert!(request.contains(CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA));
        let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
        assert!(!body.contains("abuse-test"));
        let signed_request: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            signed_request["payload"]["schema"],
            CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA
        );
        assert_eq!(
            signed_request["payload"]["authority"]["credential_exposed"],
            false
        );
        assert_eq!(
            signed_request["payload"]["authority"]["raw_peer_authority"],
            false
        );
        let request_signer = signed_request["signer_did"].as_str().unwrap().to_string();
        let signed_request_bytes = serde_json::to_vec(&signed_request).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_request_bytes,
            CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_DOMAIN,
            &[request_signer],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn content_admission_accepts_configured_federated_abuse_control_quorum() {
        let receipt_a = signed_federated_abuse_control_exchange_receipt(true, None);
        let (url_a, handle_a) = spawn_federated_abuse_control_exchange_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "abuse-control-exchange:a",
            "receipt_id": "abuse-control-receipt:a",
            "receipt": receipt_a,
        }));
        let receipt_b = signed_federated_abuse_control_exchange_receipt(true, None);
        let (url_b, handle_b) = spawn_federated_abuse_control_exchange_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "exchange_id": "abuse-control-exchange:b",
            "receipt_id": "abuse-control-receipt:b",
            "receipt": receipt_b,
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_abuse_control_exchange_config(Some(json!({
                "quorum": 2,
                "endpoints": [
                    {
                        "id": "abuse-a",
                        "url": url_a,
                        "authorization": "Bearer abuse-secret-a",
                        "timeout_secs": 5
                    },
                    {
                        "id": "abuse-b",
                        "url": url_b,
                        "authorization": "Bearer abuse-secret-b",
                        "timeout_secs": 5
                    }
                ]
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        let exchange = &admission["data"]["admission"]["federated_abuse_control_exchange"];
        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], true);
        assert_eq!(exchange["accepted"], true);
        assert_eq!(exchange["quorum"]["required"], 2);
        assert_eq!(exchange["quorum"]["endpoint_count"], 2);
        assert_eq!(exchange["quorum"]["accepted"], 2);
        assert_eq!(exchange["signed_receipt"]["verified"], true);
        assert_eq!(exchange["exchange"]["multi_endpoint"], true);
        assert_eq!(exchange["exchange"]["endpoint_count"], 2);

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["network_abuse_policy"]["network_federation"]["exchange_client"]
                ["endpoint_count"],
            2
        );
        assert_eq!(
            status["data"]["network_abuse_policy"]["network_federation"]["exchange_client"]
                ["quorum_required"],
            2
        );
        assert!(!status.to_string().contains("abuse-secret-a"));
        assert!(!status.to_string().contains("abuse-secret-b"));

        let request_a = handle_a.join().unwrap();
        let request_b = handle_b.join().unwrap();
        assert!(request_a
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer abuse-secret-a")));
        assert!(request_b
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer abuse-secret-b")));
        assert!(!request_a
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("abuse-secret-a"));
        assert!(!request_b
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("abuse-secret-b"));
    }

    #[tokio::test]
    async fn content_admission_rejects_when_configured_federated_abuse_control_rejects() {
        let receipt = signed_federated_abuse_control_exchange_receipt(
            false,
            Some("abuse threshold exceeded"),
        );
        let (url, handle) = spawn_federated_abuse_control_exchange_endpoint(json!({
            "accepted": false,
            "status": "rejected",
            "reason": "abuse threshold exceeded",
            "receipt": receipt,
        }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_abuse_control_exchange_config(Some(json!({
                "url": url,
                "authorization": "Bearer abuse-test",
                "timeout_secs": 5,
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], false);
        assert_eq!(admission["data"]["admission"]["status"], "rejected");
        assert!(admission["data"]["admission"]["reason"]
            .as_str()
            .unwrap()
            .contains("abuse threshold exceeded"));
        assert_eq!(
            admission["data"]["admission"]["federated_abuse_control_exchange"]["status"],
            "rejected"
        );
        assert_eq!(
            admission["data"]["admission"]["federated_abuse_control_exchange"]["signed_receipt"]
                ["verified"],
            true
        );
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );

        let request = handle.join().unwrap();
        assert!(request.contains(CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA));
    }

    #[tokio::test]
    async fn content_admission_rejects_configured_federated_abuse_control_quorum_failure() {
        let accepted_receipt = signed_federated_abuse_control_exchange_receipt(true, None);
        let (accepted_url, accepted_handle) =
            spawn_federated_abuse_control_exchange_endpoint(json!({
                "accepted": true,
                "status": "accepted",
                "receipt": accepted_receipt,
            }));
        let rejected_receipt = signed_federated_abuse_control_exchange_receipt(
            false,
            Some("abuse threshold exceeded"),
        );
        let (rejected_url, rejected_handle) =
            spawn_federated_abuse_control_exchange_endpoint(json!({
                "accepted": false,
                "status": "rejected",
                "reason": "abuse threshold exceeded",
                "receipt": rejected_receipt,
            }));
        let (_data_dir, _registry, _ipfs, content) =
            registry_with_content_and_ipfs_with_abuse_control_exchange_config(Some(json!({
                "quorum": 2,
                "endpoints": [
                    {"id": "abuse-a", "url": accepted_url, "timeout_secs": 5},
                    {"id": "abuse-b", "url": rejected_url, "timeout_secs": 5}
                ]
            })))
            .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        let exchange = &admission["data"]["admission"]["federated_abuse_control_exchange"];
        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], false);
        assert_eq!(admission["data"]["admission"]["status"], "rejected");
        assert!(admission["data"]["admission"]["reason"]
            .as_str()
            .unwrap()
            .contains("abuse threshold exceeded"));
        assert_eq!(exchange["accepted"], false);
        assert_eq!(exchange["quorum"]["required"], 2);
        assert_eq!(exchange["quorum"]["accepted"], 1);
        assert_eq!(exchange["quorum"]["rejected"], 1);
        assert_eq!(exchange["signed_receipt"]["verified"], true);

        let accepted_request = accepted_handle.join().unwrap();
        let rejected_request = rejected_handle.join().unwrap();
        assert!(accepted_request.contains(CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA));
        assert!(rejected_request.contains(CONTENT_FEDERATED_ABUSE_CONTROL_EXCHANGE_REQUEST_SCHEMA));
    }

    #[tokio::test]
    async fn content_admission_records_configured_storage_market_acceptance() {
        let (url, handle) = spawn_storage_market_admission_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "market_id": "market:test",
            "offer_id": "offer:123",
            "receipt": {
                "schema": "elastos.test.storage-market.offer/v1",
                "offer_id": "offer:123"
            }
        }));
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs_with_configs(
            None,
            Some(json!({
                "url": url,
                "authorization": "Bearer market-test",
                "timeout_secs": 5,
            })),
            None,
            None,
        )
        .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], true);
        assert_eq!(
            admission["data"]["admission"]["storage_market_admission"]["schema"],
            CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA
        );
        assert_eq!(
            admission["data"]["admission"]["storage_market_admission"]["accepted"],
            true
        );
        assert_eq!(
            admission["data"]["admission"]["storage_market_admission"]["offer_id"],
            "offer:123"
        );
        assert_eq!(
            admission["data"]["admission"]["storage_market_admission"]["client"]
                ["credential_exposed"],
            false
        );
        let signer_did = admission["data"]["receipt"]["signer_did"]
            .as_str()
            .unwrap()
            .to_string();
        let signed_receipt = serde_json::to_vec(&admission["data"]["receipt"]).unwrap();
        crate::crypto::verify_signed_json_envelope_against_dids(
            &signed_receipt,
            CONTENT_ADMISSION_DOMAIN,
            &[signer_did],
        )
        .unwrap();
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );

        let status = content
            .send_raw(&json!({
                "op": "status",
            }))
            .await
            .unwrap();
        assert_eq!(
            status["data"]["storage_market_admission_policy"]["production_market"]["configured"],
            true
        );
        assert_eq!(
            status["data"]["storage_market_admission_policy"]["external_admission_client"]
                ["authorization_configured"],
            true
        );
        assert!(!status.to_string().contains("market-test"));

        let request = handle.join().unwrap();
        assert!(request.contains(CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer market-test")));
        assert!(!request
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("market-test"));
    }

    #[tokio::test]
    async fn content_storage_market_admission_accepts_endpoint_quorum() {
        let (url_a, handle_a) = spawn_storage_market_admission_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "market_id": "market:a",
            "offer_id": "offer:a",
            "receipt": {
                "schema": "elastos.test.storage-market.offer/v1",
                "offer_id": "offer:a"
            }
        }));
        let (url_b, handle_b) = spawn_storage_market_admission_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "market_id": "market:b",
            "offer_id": "offer:b",
            "receipt": {
                "schema": "elastos.test.storage-market.offer/v1",
                "offer_id": "offer:b"
            }
        }));
        let client = ContentStorageMarketAdmissionClient::from_config(json!({
            "quorum": 2,
            "endpoints": [
                {
                    "id": "market-a",
                    "url": url_a,
                    "authorization": "Bearer market-secret-a",
                    "timeout_secs": 5
                },
                {
                    "id": "market-b",
                    "url": url_b,
                    "authorization": "Bearer market-secret-b",
                    "timeout_secs": 5
                }
            ]
        }))
        .unwrap();

        let decision = client
            .decide(&json!({
                "schema": CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA,
                "cid": TEST_CID,
                "estimated_content_bytes": 22,
            }))
            .await
            .unwrap();

        assert_eq!(
            decision["schema"],
            CONTENT_STORAGE_MARKET_ADMISSION_DECISION_SCHEMA
        );
        assert_eq!(decision["accepted"], true);
        assert_eq!(decision["status"], "accepted");
        assert_eq!(decision["offer_id"], "offer:a");
        assert_eq!(decision["quorum"]["required"], 2);
        assert_eq!(decision["quorum"]["endpoint_count"], 2);
        assert_eq!(decision["quorum"]["accepted"], 2);
        assert_eq!(decision["client"]["multi_endpoint"], true);
        assert_eq!(decision["client"]["endpoint_count"], 2);
        assert!(!decision.to_string().contains("market-secret-a"));
        assert!(!decision.to_string().contains("market-secret-b"));

        let request_a = handle_a.join().unwrap();
        let request_b = handle_b.join().unwrap();
        assert!(request_a
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer market-secret-a")));
        assert!(request_b
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer market-secret-b")));
        assert!(request_a.contains(CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA));
        assert!(request_b.contains(CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA));
        assert!(!request_a
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("market-secret-a"));
        assert!(!request_b
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .contains("market-secret-b"));
    }

    #[tokio::test]
    async fn content_storage_market_admission_rejects_endpoint_quorum_failure() {
        let (accepted_url, accepted_handle) = spawn_storage_market_admission_endpoint(json!({
            "accepted": true,
            "status": "accepted",
            "market_id": "market:a",
            "offer_id": "offer:a",
        }));
        let (rejected_url, rejected_handle) = spawn_storage_market_admission_endpoint(json!({
            "accepted": false,
            "status": "rejected",
            "reason": "market capacity exhausted",
        }));
        let client = ContentStorageMarketAdmissionClient::from_config(json!({
            "quorum": 2,
            "endpoints": [
                {"id": "market-a", "url": accepted_url, "timeout_secs": 5},
                {"id": "market-b", "url": rejected_url, "timeout_secs": 5}
            ]
        }))
        .unwrap();

        let decision = client
            .decide(&json!({
                "schema": CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA,
                "cid": TEST_CID,
                "estimated_content_bytes": 22,
            }))
            .await
            .unwrap();

        assert_eq!(decision["accepted"], false);
        assert_eq!(decision["status"], "rejected");
        assert_eq!(decision["quorum"]["required"], 2);
        assert_eq!(decision["quorum"]["accepted"], 1);
        assert_eq!(decision["quorum"]["rejected"], 1);
        assert!(decision["reason"]
            .as_str()
            .unwrap()
            .contains("market capacity exhausted"));

        let accepted_request = accepted_handle.join().unwrap();
        let rejected_request = rejected_handle.join().unwrap();
        assert!(accepted_request.contains(CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA));
        assert!(rejected_request.contains(CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA));
    }

    #[tokio::test]
    async fn content_admission_rejects_when_configured_storage_market_rejects() {
        let (url, handle) = spawn_storage_market_admission_endpoint(json!({
            "accepted": false,
            "status": "rejected",
            "reason": "capacity exhausted"
        }));
        let (_data_dir, _registry, _ipfs, content) = registry_with_content_and_ipfs_with_configs(
            None,
            Some(json!({
                "url": url,
                "authorization": "Bearer market-test",
                "timeout_secs": 5,
            })),
            None,
            None,
        )
        .await;

        let admission = content
            .send_raw(&json!({
                "op": "admission",
                "cid": "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
                "publisher_did": "did:key:z6Mkpublisher",
                "estimated_content_bytes": 3,
                "availability_requirements": {
                    "max_storage_bytes_per_principal": 10
                }
            }))
            .await
            .unwrap();

        assert_eq!(admission["status"], "ok");
        assert_eq!(admission["data"]["admission"]["accepted"], false);
        assert_eq!(admission["data"]["admission"]["status"], "rejected");
        assert!(admission["data"]["admission"]["reason"]
            .as_str()
            .unwrap()
            .contains("capacity exhausted"));
        assert_eq!(
            admission["data"]["admission"]["storage_market_admission"]["status"],
            "rejected"
        );
        assert_eq!(
            admission["data"]["receipt"]["payload"],
            admission["data"]["admission"]
        );

        let request = handle.join().unwrap();
        assert!(request.contains(CONTENT_STORAGE_MARKET_ADMISSION_REQUEST_SCHEMA));
    }
}
