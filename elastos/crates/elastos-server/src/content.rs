//! Content availability provider.
//!
//! This is the capsule-facing `elastos://content/*` contract. The first
//! implementation delegates bytes to the existing low-level IPFS/Kubo backend
//! and reports honest local availability status.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use elastos_common::protected_content::{
    validate_protected_content_key_envelope_algorithms, SealedObjectV1, SEALED_OBJECT_SCHEMA,
};
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;

const AVAILABILITY_RECEIPT_SCHEMA: &str = "elastos.content.availability.receipt/v1";
const AVAILABILITY_RECEIPT_DOMAIN: &str = "elastos.content.availability.receipt.v1";
const OBJECT_MANIFEST_SCHEMA: &str = "elastos.content.object.manifest/v1";
const OBJECT_MANIFEST_PATH: &str = "_elastos_object.json";
const SEALED_OBJECT_PATH: &str = "sealed.json";

pub const CONTENT_OBJECT_MANIFEST_PATH: &str = OBJECT_MANIFEST_PATH;

pub struct ContentProvider {
    data_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl ContentProvider {
    pub fn new(data_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self { data_dir, registry }
    }

    fn registry(&self) -> Result<Arc<ProviderRegistry>, ProviderError> {
        self.registry.upgrade().ok_or_else(|| {
            ProviderError::Provider("content provider registry unavailable".to_string())
        })
    }
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

pub async fn publish_directory_via_provider_with_kind(
    registry: &ProviderRegistry,
    dir: &Path,
    object_kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
) -> anyhow::Result<String> {
    publish_directory_via_provider_with_kind_and_links(
        registry,
        dir,
        object_kind,
        object_did,
        publisher_did,
        &[],
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
    });
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        request["path"] = Value::String(path.to_string());
    }

    let response = registry
        .send_raw("content", &request)
        .await
        .map_err(|err| anyhow::anyhow!("content provider unavailable: {err}"))?;
    content_response_bytes(&response)
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
            Some("ensure") => self.ensure(request).await,
            Some("repair") => self.repair(request).await,
            Some("unpublish") => self.unpublish(request).await,
            Some("status") => self.status(request),
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

        let mut ipfs_request = json!({
            "op": "cat",
            "cid": cid,
        });
        if !path.is_empty() {
            ipfs_request["path"] = Value::String(path.to_string());
        }

        let registry = self.registry()?;
        let ipfs_response = registry.send_raw("ipfs", &ipfs_request).await?;
        provider_response_ok(&ipfs_response, "content fetch")?;
        let data = ipfs_response
            .get("data")
            .and_then(|data| data.get("data"))
            .and_then(|data| data.as_str())
            .ok_or_else(|| {
                ProviderError::Provider("content backend response missing data".into())
            })?;

        let availability = self
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

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "path": path,
            "data": data,
            "availability": availability,
        })))
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

        let ipfs_response = registry.send_raw("ipfs", &ipfs_request).await?;
        let cid = provider_response_cid(&ipfs_response)?;
        let object_did = request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let local_outcome = AvailabilityOutcome::local_publish(pin);
        let outcome = if pin {
            self.ensure_network_availability(
                &registry,
                &cid,
                request,
                object_did.as_deref(),
                publisher_did.as_deref(),
                &local_outcome,
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
        })?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "receipt": receipt,
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
        let ipfs_response = registry
            .send_raw(
                "ipfs",
                &json!({
                    "op": "unpin",
                    "cid": cid,
                }),
            )
            .await?;
        provider_response_ok(&ipfs_response, "content unpublish")?;
        let receipt = self.write_receipt(ReceiptInput {
            cid: cid.to_string(),
            object_did: request
                .get("object_did")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            publisher_did: request
                .get("publisher_did")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            provider: "ipfs-provider".to_string(),
            policy: "local_unpublish".to_string(),
            status: "local_unpinned".to_string(),
            replicas: 0,
        })?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": {
                "status": "local_unpinned",
                "provider": "ipfs-provider",
                "replicas": 0,
            },
            "receipt": receipt,
        })))
    }

    async fn ensure(&self, request: &Value) -> Result<Value, ProviderError> {
        self.pin_for_availability(request, "local_ensure_pin", "local_ensure_failed")
            .await
    }

    async fn repair(&self, request: &Value) -> Result<Value, ProviderError> {
        self.pin_for_availability(request, "local_repair_pin", "local_repair_failed")
            .await
    }

    async fn pin_for_availability(
        &self,
        request: &Value,
        success_policy: &str,
        failure_policy: &str,
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
        let ipfs_response = registry
            .send_raw(
                "ipfs",
                &json!({
                    "op": "pin",
                    "cid": cid,
                }),
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
        };
        let object_did = request
            .get("object_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let publisher_did = request
            .get("publisher_did")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let outcome = if local_outcome.status == "local_pinned" {
            self.ensure_network_availability(
                &registry,
                cid,
                request,
                object_did.as_deref(),
                publisher_did.as_deref(),
                &local_outcome,
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
        })?;

        Ok(provider_ok(json!({
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "availability": outcome.to_json(),
            "receipt": receipt,
        })))
    }

    async fn ensure_network_availability(
        &self,
        registry: &ProviderRegistry,
        cid: &str,
        request: &Value,
        object_did: Option<&str>,
        publisher_did: Option<&str>,
        local: &AvailabilityOutcome,
    ) -> Result<Option<AvailabilityOutcome>, ProviderError> {
        let policy = request
            .get("availability_policy")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("network_default");
        let mut availability_request = json!({
            "op": "ensure",
            "cid": cid,
            "uri": format!("elastos://{cid}"),
            "policy": policy,
            "local": local.to_json(),
        });
        if let Some(object_did) = object_did {
            availability_request["object_did"] = Value::String(object_did.to_string());
        }
        if let Some(publisher_did) = publisher_did {
            availability_request["publisher_did"] = Value::String(publisher_did.to_string());
        }

        match registry
            .send_raw("availability", &availability_request)
            .await
        {
            Ok(response) => Ok(Some(parse_availability_provider_response(
                &response, policy, local,
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

    fn status(&self, request: &Value) -> Result<Value, ProviderError> {
        if let Some(cid) = request.get("cid").and_then(|cid| cid.as_str()) {
            if !is_valid_cid(cid) {
                return Ok(provider_error(
                    "invalid_cid",
                    "content status requires a valid CID",
                ));
            }
            if let Some(receipt) = self.latest_receipt_for_cid(cid) {
                let receipt = receipt?;
                return Ok(provider_ok(json!({
                    "cid": receipt.payload.cid,
                    "uri": receipt.payload.uri,
                    "availability": {
                        "status": receipt.payload.status,
                        "provider": receipt.payload.provider,
                        "replicas": receipt.payload.replicas,
                        "checked_at": receipt.payload.checked_at,
                    },
                    "receipt": receipt,
                })));
            }
        }

        Ok(provider_ok(json!({
            "availability": {
                "status": "unknown",
                "provider": "content-provider",
            }
        })))
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
        Ok(signed)
    }

    fn latest_receipt_for_cid(
        &self,
        cid: &str,
    ) -> Option<Result<SignedAvailabilityReceipt, ProviderError>> {
        let path = self.receipts_path();
        if !path.exists() {
            return None;
        }

        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => return Some(Err(err.into())),
        };
        let reader = std::io::BufReader::new(file);
        let mut latest = None;
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(err) => return Some(Err(err.into())),
            };
            if line.trim().is_empty() {
                continue;
            }
            let receipt: SignedAvailabilityReceipt = match serde_json::from_str(&line) {
                Ok(receipt) => receipt,
                Err(err) => {
                    return Some(Err(ProviderError::Provider(format!(
                        "content receipt ledger decode failed: {err}"
                    ))))
                }
            };
            if receipt.payload.cid == cid {
                if let Err(err) = verify_signed_receipt(&receipt) {
                    return Some(Err(err));
                }
                latest = Some(receipt);
            }
        }
        latest.map(Ok)
    }

    fn receipts_path(&self) -> PathBuf {
        self.data_dir
            .join("ElastOS")
            .join("SystemServices")
            .join("Content")
            .join("availability-receipts.jsonl")
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
    pub checked_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedAvailabilityReceipt {
    pub payload: AvailabilityReceipt,
    pub signature: String,
    pub signer_did: String,
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
}

#[derive(Debug, Clone)]
struct AvailabilityOutcome {
    provider: String,
    policy: String,
    status: String,
    replicas: u32,
    reason: Option<String>,
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
            }
        } else {
            Self {
                provider: "ipfs-provider".to_string(),
                policy: "local_add".to_string(),
                status: "local_unpinned".to_string(),
                replicas: 0,
                reason: None,
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
        }
    }

    fn to_json(&self) -> Value {
        let mut availability = json!({
            "status": self.status,
            "provider": self.provider,
            "replicas": self.replicas,
        });
        if let Some(reason) = &self.reason {
            availability["reason"] = Value::String(reason.clone());
        }
        availability
    }
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

fn parse_availability_provider_response(
    response: &Value,
    requested_policy: &str,
    local: &AvailabilityOutcome,
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
        "network_available" if replicas > 0 => AvailabilityOutcome {
            provider: provider.to_string(),
            policy: policy.to_string(),
            status: status.to_string(),
            replicas,
            reason: None,
        },
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
        _ => AvailabilityOutcome::repair_needed(
            provider,
            policy,
            local.replicas,
            "availability provider returned an unsupported status".to_string(),
        ),
    }
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

fn content_response_bytes(response: &Value) -> anyhow::Result<Vec<u8>> {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("content fetch failed: {message}");
    }
    let data = response
        .get("data")
        .and_then(|data| data.get("data"))
        .and_then(|data| data.as_str())
        .ok_or_else(|| anyhow::anyhow!("No data in content provider response"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| anyhow::anyhow!("Content provider returned invalid base64: {err}"))
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
        "capsule" | "directory" | "document" | "release" | "sealed" | "share" | "site" => {
            Ok(kind.to_string())
        }
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

    async fn registry_with_content_and_ipfs() -> (
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
        let content = Arc::new(ContentProvider::new(
            data_dir.path().to_path_buf(),
            Arc::downgrade(&registry),
        ));
        registry.register(content.clone()).await;
        registry
            .register_sub_provider("content", content.clone())
            .await
            .unwrap();
        (data_dir, registry, ipfs, content)
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
    async fn content_publish_uses_registered_availability_provider() {
        let (_data_dir, registry, _ipfs, content) = registry_with_content_and_ipfs().await;
        let availability = Arc::new(MockAvailabilityProvider {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(provider_ok(json!({
                "availability": {
                    "status": "network_available",
                    "provider": "elacity-supernode",
                    "policy": "smartweb_default",
                    "replicas": 3
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

        let requests = availability.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "ensure");
        assert_eq!(requests[0]["cid"], TEST_CID);
        assert_eq!(requests[0]["uri"], format!("elastos://{TEST_CID}"));
        assert_eq!(requests[0]["local"]["status"], "local_pinned");
        assert_eq!(requests[0]["object_did"], "did:key:z6Mkobject");
        assert_eq!(requests[0]["publisher_did"], "did:key:z6Mkpublisher");
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
            response["data"]["receipt"]["payload"]["status"],
            "repair_needed"
        );
    }

    #[tokio::test]
    async fn content_publish_file_wraps_ipfs_bytes_with_receipt() {
        let (_data_dir, registry, ipfs, _content) = registry_with_content_and_ipfs().await;
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
    }

    #[tokio::test]
    async fn content_fetch_wraps_ipfs_cat() {
        let (_data_dir, registry, _ipfs, _content) = registry_with_content_and_ipfs().await;
        let bytes = fetch_bytes_via_provider(&registry, TEST_CID, Some("capsule.json"))
            .await
            .unwrap();

        assert_eq!(bytes, b"hello content");
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
        assert_eq!(
            status["data"]["receipt"]["payload"]["schema"],
            AVAILABILITY_RECEIPT_SCHEMA
        );
    }
}
