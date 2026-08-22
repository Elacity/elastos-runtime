//! Inactive protected-content Runtime provider plane.
//!
//! This module registers the canonical `custody` registry route as Runtime-owned
//! and inactive, scans the durable Runtime release journal on startup, and
//! evaluates rights through the existing `chain` registry route. It does not
//! replace the provisional `key`/`rights`/`drm`/`decrypt` product path or expose
//! provider topology.

#[cfg(test)]
mod tests;

use std::fs;
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use base64::Engine as _;
use ed25519_dalek::Signer as _;
use elastos_protected_content_contracts::{
    validate_custody_epoch_against_pool_at, CanonicalContract,
    CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIssuerKeyV1, Digest32,
    KeyReleaseRequestV1, NodeCustodyPublicKeyV1, NodePublicKey,
    RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1, RecipientPublicKeyBytesV1,
    RightsEvaluationEvidenceRequestV1, RightsPolicyBodyV1, RuntimeOperationIssuerKeyV1,
    RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
    SignedCustodyCommitteeAuthorizationV1, SignedCustodyEpochV1, SignedCustodyPoolV1,
    SignedRuntimeReleaseOperationV1, WalletSignedRightsRequestV1,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, RightsProviderRequestV1, RightsProviderResponseV1,
};
#[cfg(test)]
use elastos_protected_content_provider_contracts::{
    DecryptProviderRequestOpV1, DecryptProviderRequestV1, DecryptProviderResponseV1,
};
use elastos_protected_content_rights::{
    PrivateCustodyRightsRequestV1, CHAIN_PROVIDER_ID, CHAIN_RIGHTS_EVIDENCE_OP,
};
#[cfg(test)]
use elastos_protected_content_runtime::RuntimeDecryptProvider;
use elastos_protected_content_runtime::RuntimeProviderCallError;
use elastos_protected_content_runtime::{
    resolve_runtime_mint_selected_nodes, PersistedRuntimeReleaseOperation,
    RuntimeContentAvailabilityRequirement, RuntimeMintConfiguredCustodyProvider,
    RuntimeMintCoordinatorError, RuntimeMintJournal, RuntimeReleaseAuditRecord,
    RuntimeReleaseJournal, RuntimeReleaseJournalError, RuntimeVerifiedContentAvailability,
};
use elastos_runtime::provider::bridge::{ProviderBridge, ProviderConfig};
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer, ResourceRequest, ResourceResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest as _;

pub(crate) const CUSTODY_PROVIDER_ID: &str = "custody";
pub(crate) const RUNTIME_PROVIDER_ID: &str = "runtime";
const CONTENT_PROVIDER_ID: &str = "content";
const PROTECTED_CONTENT_ROOT: &str = "protected-content";
const CUSTODY_COMPOSITION_CONFIG_FILE: &str = "protected-content/custody-composition.json";
const RUNTIME_MINT_JOURNAL_ROOT: &str = "protected-content/runtime-mint";
const CUSTODY_COMPOSITION_SCHEMA_V1: &str = "elastos.protected-content.custody-composition/v1";
const MAX_CUSTODY_COMPOSITION_BYTES: usize = 64 * 1024;
const MAX_CUSTODY_COMPOSITION_BLOB_BYTES: usize = 16 * 1024;
const MAX_CUSTODY_COMPOSITION_PEER_DID_BYTES: usize = 256;
const PROTECTED_CONTENT_OBJECT_KIND: &str = "protected-content";
const PROTECTED_CONTENT_IDENTITY_PATH: &str = "protected-content/v1/identity.bin";
const PROTECTED_CONTENT_INIT_PATH: &str = "protected-content/v1/init.mp4";
const PROTECTED_CONTENT_SEGMENTS_PREFIX: &str = "protected-content/v1/segments/";
const PROTECTED_CONTENT_SEGMENTS_SUFFIX: &str = ".m4s";
const PROTECTED_CONTENT_AVAILABLE_STATUS: &str = "network_available";
const CONTENT_AVAILABILITY_RECEIPT_SCHEMA: &str = "elastos.content.availability.receipt/v1";
const CONTENT_AVAILABILITY_RECEIPT_DOMAIN: &str = "elastos.content.availability.receipt.v1";
#[cfg(test)]
const DECRYPT_PROVIDER_ID: &str = "decrypt";
const INACTIVE_CUSTODY_ROOT: &str = "protected-content/custody-provider/inactive";
const PROVIDER_INVOCATION_SCHEMA_V1: &str = "elastos.provider.invocation/v1";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RuntimeCustodyRouteTransportConfig {
    Local,
    CarrierPeerDid { peer_did: String },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeCustodyRouteBindingConfig {
    node_public_key_base64: String,
    owner_state_root_base64: String,
    transport: RuntimeCustodyRouteTransportConfig,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeCustodyCompositionConfigFile {
    schema: String,
    expected_policy_authority_base64: String,
    expected_committee_authorization_identity_base64: String,
    signed_pool_base64: String,
    signed_epoch_base64: String,
    signed_committee_authorization_base64: String,
    routes: Vec<RuntimeCustodyRouteBindingConfig>,
}

struct RuntimeValidatedCustodyRouteBinding {
    owner_state_root: Digest32,
    transport: ProviderInvocationTransport,
}

struct RuntimeValidatedCustodyCompositionConfig {
    expected_policy_authority: CustodyEpochIssuerKeyV1,
    expected_authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
    signed_pool: SignedCustodyPoolV1,
    signed_epoch: SignedCustodyEpochV1,
    signed_committee_authorization: SignedCustodyCommitteeAuthorizationV1,
    routes: [RuntimeValidatedCustodyRouteBinding; 3],
}

struct RuntimeCustodyCompositionNode {
    node_public_key: NodePublicKey,
    custody_public_key: NodeCustodyPublicKeyV1,
    owner_state_root: Digest32,
    adapter: RuntimeCustodyRegistryAdapter,
}

pub(crate) struct RuntimeCustodyComposition {
    expected_policy_authority: CustodyEpochIssuerKeyV1,
    expected_authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
    signed_pool: SignedCustodyPoolV1,
    signed_epoch: SignedCustodyEpochV1,
    signed_committee_authorization: SignedCustodyCommitteeAuthorizationV1,
    nodes: [RuntimeCustodyCompositionNode; 3],
}

impl std::fmt::Debug for RuntimeCustodyComposition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeCustodyComposition")
            .field("expected_policy_authority", &"[redacted]")
            .field("expected_authorization_identity", &"[redacted]")
            .field("signed_pool", &"[redacted]")
            .field("signed_epoch", &"[redacted]")
            .field("signed_committee_authorization", &"[redacted]")
            .field("node_count", &self.nodes.len())
            .finish()
    }
}

impl RuntimeCustodyComposition {
    pub(crate) fn configured_nodes(
        &self,
    ) -> Result<[RuntimeMintConfiguredCustodyProvider<'_>; 3], RuntimeMintCoordinatorError> {
        Ok([
            RuntimeMintConfiguredCustodyProvider::new(
                self.nodes[0].node_public_key,
                self.nodes[0].custody_public_key,
                self.nodes[0].owner_state_root,
                &self.nodes[0].adapter,
            )?,
            RuntimeMintConfiguredCustodyProvider::new(
                self.nodes[1].node_public_key,
                self.nodes[1].custody_public_key,
                self.nodes[1].owner_state_root,
                &self.nodes[1].adapter,
            )?,
            RuntimeMintConfiguredCustodyProvider::new(
                self.nodes[2].node_public_key,
                self.nodes[2].custody_public_key,
                self.nodes[2].owner_state_root,
                &self.nodes[2].adapter,
            )?,
        ])
    }
}

struct InactiveCustodyProvider {
    bridge: Arc<ProviderBridge>,
    registry: Weak<ProviderRegistry>,
}

impl InactiveCustodyProvider {
    fn new(bridge: Arc<ProviderBridge>, registry: Weak<ProviderRegistry>) -> Self {
        Self { bridge, registry }
    }

    fn strip_runtime_invocation(&self, request: &Value) -> Result<Value, ProviderError> {
        let mut value = request.clone();
        let object = value
            .as_object_mut()
            .ok_or_else(|| ProviderError::Provider("custody request must be an object".into()))?;
        if object.contains_key("_runtime_transfer") {
            return Err(ProviderError::Provider(
                "custody request contains an unsupported transfer envelope".into(),
            ));
        }
        if object.contains_key("chain_data") || object.contains_key("evidence") {
            return Err(ProviderError::Provider(
                "custody request contains unsupported injected evidence".into(),
            ));
        }
        if object.contains_key("carrier") {
            return Err(ProviderError::Provider(
                "custody request contains unsupported injected carrier data".into(),
            ));
        }
        let op = object
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("custody request is missing op".into()))?
            .to_owned();
        if matches!(
            op.as_str(),
            "evaluate_rights" | "prepare_evidence" | "settle_evidence"
        ) {
            return Err(ProviderError::Provider(
                "custody request op is private to the node-hosted wrapper".into(),
            ));
        }
        match op.as_str() {
            "status" | "provision_node_share" | "release_contribution" | "evaluate" => {}
            _ => {
                return Err(ProviderError::Provider(
                    "custody request op is unsupported".into(),
                ));
            }
        }
        let envelope = object.remove("_runtime_invocation").ok_or_else(|| {
            ProviderError::Provider("custody request is missing runtime envelope".into())
        })?;
        let envelope = envelope.as_object().ok_or_else(|| {
            ProviderError::Provider("custody request runtime envelope is invalid".into())
        })?;
        let transport = envelope
            .get("transport")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Provider("custody request runtime envelope is invalid".into())
            })?;
        if !matches!(
            transport,
            "runtime-local-provider-plane" | "carrier-provider-plane"
        ) {
            return Err(ProviderError::Provider(
                "custody request runtime envelope is invalid".into(),
            ));
        }
        if envelope.len() != 11
            || envelope.get("schema").and_then(Value::as_str) != Some(PROVIDER_INVOCATION_SCHEMA_V1)
            || envelope.get("source").and_then(Value::as_str) != Some(RUNTIME_PROVIDER_ID)
            || envelope.get("target").and_then(Value::as_str) != Some(CUSTODY_PROVIDER_ID)
            || envelope.get("op").and_then(Value::as_str) != Some(op.as_str())
            || envelope.get("capability").and_then(Value::as_str)
                != Some(&format!(
                    "provider:{RUNTIME_PROVIDER_ID}->{CUSTODY_PROVIDER_ID}:{op}"
                ))
            || envelope.get("carrier") != Some(&Value::Null)
            || envelope.get("transfer").and_then(Value::as_str) != Some("json")
            || envelope.get("range") != Some(&Value::Null)
            || envelope.get("progress") != Some(&Value::Null)
            || envelope.get("abi")
                != Some(&json!({
                    "schema": "elastos.provider.transfer-abi/v1",
                    "transfer": "json",
                    "transport": transport,
                    "range_supported": false,
                    "progress_supported": false,
                    "progress_mode": "none",
                    "transport_native_stream": false,
                    "backpressure": "not_applicable",
                    "cancel_supported": false,
                }))
        {
            return Err(ProviderError::Provider(
                "custody request runtime envelope is invalid".into(),
            ));
        }
        Ok(value)
    }

    async fn send_private_child_request(
        &self,
        request: &PrivateCustodyRightsRequestV1,
    ) -> Result<Value, ProviderError> {
        let request = serde_json::to_value(request)
            .map_err(|_| ProviderError::Provider("custody private request is invalid".into()))?;
        self.bridge
            .send_raw(&request)
            .await
            .map_err(|error| ProviderError::Provider(error.to_string()))
    }

    fn private_ok_data(&self, response: Value) -> Result<Value, ProviderError> {
        match response.get("status").and_then(Value::as_str) {
            Some("ok") => response.get("data").cloned().ok_or_else(|| {
                ProviderError::Provider("custody private response is missing data".into())
            }),
            Some("error") => Err(ProviderError::Provider(
                "custody private request was rejected".into(),
            )),
            _ => Err(ProviderError::Provider(
                "custody private response has unsupported status".into(),
            )),
        }
    }

    fn normalize_public_status(&self, response: Value) -> Result<Value, ProviderError> {
        let data = self.private_ok_data(response)?;
        let object = data
            .as_object()
            .ok_or_else(|| ProviderError::Provider("custody status response is invalid".into()))?;
        let provider = object
            .get("provider")
            .cloned()
            .ok_or_else(|| ProviderError::Provider("custody status response is invalid".into()))?;
        let version = object
            .get("version")
            .cloned()
            .ok_or_else(|| ProviderError::Provider("custody status response is invalid".into()))?;
        let configured = object
            .get("configured")
            .cloned()
            .ok_or_else(|| ProviderError::Provider("custody status response is invalid".into()))?;
        let request_schema = object
            .get("request_schema")
            .cloned()
            .ok_or_else(|| ProviderError::Provider("custody status response is invalid".into()))?;
        let response_schema = object
            .get("response_schema")
            .cloned()
            .ok_or_else(|| ProviderError::Provider("custody status response is invalid".into()))?;
        Ok(json!({
            "status": "ok",
            "data": {
                "provider": provider,
                "version": version,
                "configured": configured,
                "supported_operations": [
                    "status",
                    "provision_node_share",
                    "release_contribution",
                    "evaluate",
                ],
                "request_schema": request_schema,
                "response_schema": response_schema,
            }
        }))
    }
}

impl std::fmt::Debug for InactiveCustodyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InactiveCustodyProvider")
            .field("bridge", &"<redacted>")
            .finish()
    }
}

struct RuntimeCustodyRegistryAdapter {
    registry: Arc<ProviderRegistry>,
    transport: ProviderInvocationTransport,
}

impl RuntimeCustodyRegistryAdapter {
    fn new(registry: Arc<ProviderRegistry>, transport: ProviderInvocationTransport) -> Self {
        Self {
            registry,
            transport,
        }
    }

    async fn invoke_rights(
        &self,
        request: &RightsProviderRequestV1,
    ) -> Result<RightsProviderResponseV1, RuntimeProviderCallError> {
        let request_value = serde_json::from_slice(
            &request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?,
        )
        .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
        let response_value = invoke_json_provider_with_transport(
            self.registry.as_ref(),
            CUSTODY_PROVIDER_ID,
            "evaluate",
            request_value,
            self.transport.clone(),
        )
        .await
        .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
        RightsProviderResponseV1::from_json_slice(
            &serde_json::to_vec(&response_value)
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?,
        )
        .map_err(|_| RuntimeProviderCallError::NoExactResult)
    }

    async fn invoke_custody(
        &self,
        op: &'static str,
        request: &elastos_protected_content_provider_contracts::CustodyProviderRequestV1,
    ) -> Result<
        elastos_protected_content_provider_contracts::CustodyProviderResponseV1,
        RuntimeProviderCallError,
    > {
        let request_value = serde_json::from_slice(
            &request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?,
        )
        .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
        let response_value = invoke_json_provider_with_transport(
            self.registry.as_ref(),
            CUSTODY_PROVIDER_ID,
            op,
            request_value,
            self.transport.clone(),
        )
        .await
        .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
        elastos_protected_content_provider_contracts::CustodyProviderResponseV1::from_json_slice(
            &serde_json::to_vec(&response_value)
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?,
        )
        .map_err(|_| RuntimeProviderCallError::NoExactResult)
    }
}

impl std::fmt::Debug for RuntimeCustodyRegistryAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeCustodyRegistryAdapter")
            .field("registry", &"<redacted>")
            .field("transport", &"<redacted>")
            .finish()
    }
}

#[async_trait::async_trait]
impl elastos_protected_content_runtime::RuntimeRightsProvider for RuntimeCustodyRegistryAdapter {
    async fn evaluate_rights(
        &self,
        request: &RightsProviderRequestV1,
    ) -> Result<RightsProviderResponseV1, RuntimeProviderCallError> {
        self.invoke_rights(request).await
    }
}

#[async_trait::async_trait]
impl elastos_protected_content_runtime::RuntimeCustodyProvider for RuntimeCustodyRegistryAdapter {
    async fn release_contribution(
        &self,
        request: &elastos_protected_content_provider_contracts::CustodyProviderRequestV1,
    ) -> Result<
        elastos_protected_content_provider_contracts::CustodyProviderResponseV1,
        RuntimeProviderCallError,
    > {
        self.invoke_custody("release_contribution", request).await
    }

    async fn provision_node_share(
        &self,
        request: &elastos_protected_content_provider_contracts::CustodyProviderRequestV1,
    ) -> Result<
        elastos_protected_content_provider_contracts::CustodyProviderResponseV1,
        RuntimeProviderCallError,
    > {
        self.invoke_custody("provision_node_share", request).await
    }
}

#[async_trait::async_trait]
impl Provider for InactiveCustodyProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "inactive custody provider does not expose resources".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![CUSTODY_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        "inactive-custody-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let request = self.strip_runtime_invocation(request)?;
        if request.get("op").and_then(Value::as_str) == Some("status") {
            let response = self
                .bridge
                .send_raw(&request)
                .await
                .map_err(|error| ProviderError::Provider(error.to_string()))?;
            return self.normalize_public_status(response);
        }
        if request.get("op").and_then(Value::as_str) == Some("evaluate") {
            let prepare = PrivateCustodyRightsRequestV1::new_prepare(request.clone());
            let prepare_response = self.send_private_child_request(&prepare).await?;
            let chain_request = self.private_ok_data(prepare_response)?;
            let registry = self.registry.upgrade().ok_or_else(|| {
                ProviderError::Provider("node-local custody registry is unavailable".into())
            })?;
            let chain_data = invoke_json_provider_with_transport(
                registry.as_ref(),
                CHAIN_PROVIDER_ID,
                CHAIN_RIGHTS_EVIDENCE_OP,
                chain_request,
                ProviderInvocationTransport::Local,
            )
            .await
            .map_err(ProviderError::Provider)?;
            let settle = PrivateCustodyRightsRequestV1::new_settle(request, chain_data);
            return self.send_private_child_request(&settle).await;
        }
        self.bridge
            .send_raw(&request)
            .await
            .map_err(|error| ProviderError::Provider(error.to_string()))
    }

    async fn shutdown(&self) -> Result<(), ProviderError> {
        self.bridge
            .shutdown()
            .await
            .map_err(|error| ProviderError::Provider(error.to_string()))
    }
}

/// Exact typed inputs for one Profile-authorized Runtime release operation.
///
/// This is an in-memory assembly value. It does not claim replay or persist
/// any provider result; the Runtime release journal owns both responsibilities.
#[derive(Clone)]
pub(crate) struct RuntimeReleaseOperationAssemblyInput {
    pub(crate) rights_request: WalletSignedRightsRequestV1,
    pub(crate) release_request: KeyReleaseRequestV1,
    pub(crate) recipient_public_key: RecipientPublicKeyBytesV1,
    pub(crate) recipient_identity: RecipientKeyIdentityV1,
    pub(crate) policy_body: RightsPolicyBodyV1,
    pub(crate) evidence_request: RightsEvaluationEvidenceRequestV1,
    pub(crate) custody_epoch: SignedCustodyEpochV1,
    pub(crate) audit_request_id: RuntimeReleaseAuditIdV1,
    pub(crate) issued_at: u64,
    pub(crate) expires_at: u64,
}

/// Assemble and verify one Profile-authorized Runtime release operation.
///
/// The device key is loaded only from the existing local identity. This helper
/// never creates keys, dispatches providers, or claims replay.
pub(crate) fn assemble_protected_content_runtime_release_operation(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    proof_binding_id: &str,
    input: RuntimeReleaseOperationAssemblyInput,
    now_unix_seconds: u64,
) -> anyhow::Result<SignedRuntimeReleaseOperationV1> {
    let (device_key, _) =
        crate::collaboration_profile_authority::load_existing_device_signing_key(data_dir)?
            .ok_or_else(|| anyhow::anyhow!("local Runtime device signing key is missing"))?;
    let runtime_issuer = RuntimeOperationIssuerKeyV1::new(device_key.verifying_key().to_bytes())
        .map_err(|_| anyhow::anyhow!("local Runtime device signing key is invalid"))?;
    if input.release_request.recipient() != &input.recipient_identity {
        anyhow::bail!("release request recipient does not match recipient identity");
    }
    let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
        input.rights_request.request().binding().clone(),
        input.rights_request.request().action(),
        input.recipient_public_key,
        input.recipient_identity,
        runtime_issuer,
        input.issued_at,
        input.expires_at,
    )
    .map_err(|_| anyhow::anyhow!("recipient authorization statement is invalid"))?;
    let recipient_authorization =
        crate::collaboration_profile_authority::sign_protected_content_recipient_key_authorization(
            data_dir,
            principal_id,
            localhost_root,
            proof_binding_id,
            authorization_statement,
            now_unix_seconds,
        )?;
    let statement = RuntimeReleaseOperationStatementV1::new(
        runtime_issuer,
        input.rights_request,
        input.release_request,
        input.recipient_public_key,
        recipient_authorization,
        input.policy_body,
        input.evidence_request,
        input.custody_epoch,
        input.audit_request_id,
        input.issued_at,
        input.expires_at,
    )
    .map_err(|_| anyhow::anyhow!("Runtime release operation statement is invalid"))?;
    let signed = SignedRuntimeReleaseOperationV1::new(
        statement.clone(),
        device_key
            .sign(
                &statement
                    .canonical_bytes()
                    .map_err(|_| anyhow::anyhow!("Runtime release operation is not canonical"))?,
            )
            .to_bytes()
            .to_vec(),
    )
    .map_err(|_| anyhow::anyhow!("Runtime release operation signature is invalid"))?;
    signed
        .verify(runtime_issuer, now_unix_seconds)
        .map_err(|_| anyhow::anyhow!("Runtime release operation verification failed"))?;
    Ok(signed)
}

#[cfg(test)]
struct RuntimeDecryptRegistryAdapter {
    registry: Arc<ProviderRegistry>,
}

#[cfg(test)]
impl RuntimeDecryptRegistryAdapter {
    fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self { registry }
    }
}

#[cfg(test)]
fn decrypt_provider_op(request: &DecryptProviderRequestV1) -> &'static str {
    match request.op() {
        DecryptProviderRequestOpV1::PrepareRecipient => "prepare_recipient",
        DecryptProviderRequestOpV1::OpenViewerSession => "open_viewer_session",
        DecryptProviderRequestOpV1::ReadViewerMediaPart => "read_viewer_media_part",
        DecryptProviderRequestOpV1::CancelPreparedRecipient => "cancel_prepared_recipient",
        DecryptProviderRequestOpV1::CloseViewerSession => "close_viewer_session",
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl RuntimeDecryptProvider for RuntimeDecryptRegistryAdapter {
    async fn prepare_recipient(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
        invoke_decrypt_provider(self.registry.as_ref(), request).await
    }

    async fn open_viewer_session(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
        invoke_decrypt_provider(self.registry.as_ref(), request).await
    }

    async fn read_viewer_media_part(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
        invoke_decrypt_provider(self.registry.as_ref(), request).await
    }

    async fn cancel_prepared_recipient(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
        invoke_decrypt_provider(self.registry.as_ref(), request).await
    }

    async fn close_viewer_session(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
        invoke_decrypt_provider(self.registry.as_ref(), request).await
    }
}

#[cfg(test)]
async fn invoke_decrypt_provider(
    registry: &ProviderRegistry,
    request: &DecryptProviderRequestV1,
) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
    let request_value: Value = serde_json::from_slice(&request_bytes)
        .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
    let response_value = invoke_json_provider(
        registry,
        DECRYPT_PROVIDER_ID,
        decrypt_provider_op(request),
        request_value,
    )
    .await
    .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
    let response_bytes =
        serde_json::to_vec(&response_value).map_err(|_| RuntimeProviderCallError::NoExactResult)?;
    DecryptProviderResponseV1::from_json_slice(&response_bytes)
        .map_err(|_| RuntimeProviderCallError::NoExactResult)
}

pub fn runtime_release_journal(data_dir: &Path) -> RuntimeReleaseJournal {
    RuntimeReleaseJournal::new(data_dir.join("protected-content").join("runtime-release"))
}

pub(crate) fn runtime_mint_journal(data_dir: &Path) -> RuntimeMintJournal {
    RuntimeMintJournal::new(data_dir.join(RUNTIME_MINT_JOURNAL_ROOT))
}

pub fn list_unresolved_runtime_releases(
    data_dir: &Path,
) -> Result<Vec<PersistedRuntimeReleaseOperation>, RuntimeReleaseJournalError> {
    runtime_release_journal(data_dir).list_unresolved()
}

pub fn unresolved_release_audit_records(
    data_dir: &Path,
) -> Result<Vec<RuntimeReleaseAuditRecord>, RuntimeReleaseJournalError> {
    list_unresolved_runtime_releases(data_dir)?
        .iter()
        .map(PersistedRuntimeReleaseOperation::audit_record)
        .collect()
}

pub async fn register_inactive_custody_provider(
    registry: &Arc<ProviderRegistry>,
    binary_path: &Path,
    data_dir: &Path,
) -> anyhow::Result<()> {
    let state_root = inactive_custody_state_root(data_dir);
    validate_inactive_custody_state_root(&state_root)?;
    let base_path = state_root
        .to_str()
        .ok_or_else(|| {
            invalid_inactive_custody_config("inactive custody provider root must be valid UTF-8")
        })?
        .to_owned();
    let bridge_config = ProviderConfig {
        base_path,
        ..Default::default()
    };
    let bridge = elastos_runtime::provider::ProviderBridge::spawn(binary_path, bridge_config)
        .await
        .map_err(|error| anyhow::anyhow!("failed to spawn inactive custody provider: {error}"))?;
    let provider: Arc<dyn Provider> = Arc::new(InactiveCustodyProvider::new(
        Arc::new(bridge),
        Arc::downgrade(registry),
    ));
    if let Err(error) = register_inactive_custody_sub_provider(registry, provider.clone()).await {
        if let Err(shutdown_error) = provider.shutdown().await {
            return Err(anyhow::anyhow!(
                "failed to register inactive custody route: {error}; failed to settle rejected inactive custody provider: {shutdown_error}"
            ));
        }
        return Err(anyhow::anyhow!(
            "failed to register inactive custody route: {error}"
        ));
    }
    Ok(())
}

async fn register_inactive_custody_sub_provider(
    registry: &ProviderRegistry,
    provider: Arc<dyn Provider>,
) -> Result<(), ProviderError> {
    registry
        .register_sub_provider(CUSTODY_PROVIDER_ID, provider)
        .await
}

fn inactive_custody_state_root(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(INACTIVE_CUSTODY_ROOT)
}

fn protected_content_root(data_dir: &Path) -> PathBuf {
    data_dir.join(PROTECTED_CONTENT_ROOT)
}

fn runtime_custody_composition_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CUSTODY_COMPOSITION_CONFIG_FILE)
}

fn validate_inactive_custody_state_root(root: &Path) -> anyhow::Result<()> {
    validate_owner_only_directory(root, "inactive custody provider root")
}

fn invalid_inactive_custody_config(reason: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("inactive custody provider configuration is missing or unsafe: {reason}")
}

fn validate_owner_only_directory(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        let _ = error;
        invalid_inactive_custody_config(format!("{label} is unavailable"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "{}",
            invalid_inactive_custody_config(format!("{label} must be an owner-only directory"))
        );
    }
    validate_owner_only_metadata_with_error(
        label,
        &metadata,
        false,
        invalid_inactive_custody_config,
    )
}

#[cfg(unix)]
fn validate_owner_only_metadata_with_error(
    label: &str,
    metadata: &fs::Metadata,
    require_single_link: bool,
    error_fn: fn(String) -> anyhow::Error,
) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let mode = metadata.permissions().mode() & 0o777;
    let expected_mask = if metadata.is_dir() { 0o700 } else { 0o600 };
    if metadata.uid() != unsafe { libc::geteuid() } || mode & 0o077 != 0 {
        anyhow::bail!("{}", error_fn(format!("{label} must be owner-only")));
    }
    if require_single_link && metadata.nlink() != 1 {
        anyhow::bail!("{}", error_fn(format!("{label} must not be hard-linked")));
    }
    if mode == 0 {
        anyhow::bail!("{}", error_fn(format!("{label} must not be inaccessible")));
    }
    if mode & expected_mask != mode {
        anyhow::bail!(
            "{}",
            error_fn(format!("{label} has an unsupported owner mode"))
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_only_metadata_with_error(
    _label: &str,
    _metadata: &fs::Metadata,
    _require_single_link: bool,
    _error_fn: fn(String) -> anyhow::Error,
) -> anyhow::Result<()> {
    anyhow::bail!("owner-only protected-content config validation is unsupported on this platform")
}

fn invalid_custody_composition_config(reason: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("protected-content custody composition config is missing or unsafe: {reason}")
}

fn validate_owner_only_protected_content_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let root = protected_content_root(data_dir);
    let metadata = fs::symlink_metadata(&root).map_err(|_| {
        invalid_custody_composition_config("protected-content parent is unavailable")
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config(
                "protected-content parent must be an owner-only directory"
            )
        );
    }
    validate_owner_only_metadata_with_error(
        "protected-content parent",
        &metadata,
        false,
        invalid_custody_composition_config,
    )?;
    Ok(root)
}

#[cfg(unix)]
fn read_owner_only_config_bytes(path: &Path, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path).map_err(|_| {
        invalid_custody_composition_config("custody-composition file is unavailable")
    })?;
    let metadata = file.metadata().map_err(|_| {
        invalid_custody_composition_config("custody-composition file metadata is unavailable")
    })?;
    if !metadata.is_file() {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config("custody-composition file must be a regular file")
        );
    }
    validate_owner_only_metadata_with_error(
        "custody-composition file",
        &metadata,
        true,
        invalid_custody_composition_config,
    )?;
    let mut bytes = Vec::with_capacity(max_bytes.min(4096));
    file.by_ref()
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config("custody-composition file exceeds bounds")
        );
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_owner_only_config_bytes(path: &Path, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
    let _ = (path, max_bytes);
    anyhow::bail!("owner-only protected-content config validation is unsupported on this platform")
}

fn canonical_custody_composition_config_bytes(
    config: &RuntimeCustodyCompositionConfigFile,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(config)?)?)
}

fn decode_canonical_base64_bytes(
    encoded: &str,
    max_decoded_bytes: usize,
    field: &str,
) -> anyhow::Result<Vec<u8>> {
    let max_encoded_bytes = max_decoded_bytes.div_ceil(3) * 4;
    if encoded.is_empty() || encoded.len() > max_encoded_bytes {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config(format!("{field} base64 has invalid length"))
        );
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| invalid_custody_composition_config(format!("{field} base64 is invalid")))?;
    if decoded.is_empty()
        || decoded.len() > max_decoded_bytes
        || base64::engine::general_purpose::STANDARD.encode(&decoded) != encoded
    {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config(format!("{field} base64 is not canonical"))
        );
    }
    Ok(decoded)
}

fn decode_canonical_contract_base64<T: CanonicalContract>(
    encoded: &str,
    max_decoded_bytes: usize,
    field: &str,
) -> anyhow::Result<T> {
    let bytes = decode_canonical_base64_bytes(encoded, max_decoded_bytes, field)?;
    T::from_canonical_bytes(&bytes).map_err(|error| {
        invalid_custody_composition_config(format!("{field} canonical bytes are invalid: {error}"))
    })
}

fn decode_digest32_base64(encoded: &str, field: &str) -> anyhow::Result<Digest32> {
    let bytes = decode_canonical_base64_bytes(encoded, 32, field)?;
    let digest: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid_custody_composition_config(format!("{field} must be 32 bytes")))?;
    Ok(Digest32::new(digest))
}

fn decode_custody_epoch_issuer_base64(
    encoded: &str,
    field: &str,
) -> anyhow::Result<CustodyEpochIssuerKeyV1> {
    let bytes = decode_canonical_base64_bytes(encoded, 32, field)?;
    let issuer: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid_custody_composition_config(format!("{field} must be 32 bytes")))?;
    CustodyEpochIssuerKeyV1::new(issuer).map_err(|error| {
        invalid_custody_composition_config(format!("{field} bytes are invalid: {error}"))
    })
}

fn decode_node_public_key_base64(encoded: &str, field: &str) -> anyhow::Result<NodePublicKey> {
    let bytes = decode_canonical_base64_bytes(encoded, 32, field)?;
    let public_key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid_custody_composition_config(format!("{field} must be 32 bytes")))?;
    NodePublicKey::new(public_key).map_err(|error| {
        invalid_custody_composition_config(format!("{field} bytes are invalid: {error}"))
    })
}

fn decode_canonical_peer_did(peer_did: &str) -> anyhow::Result<String> {
    if peer_did.is_empty() || peer_did.len() > MAX_CUSTODY_COMPOSITION_PEER_DID_BYTES {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config("carrier peer DID is out of bounds")
        );
    }
    let verifying_key = crate::crypto::decode_did_key(peer_did).map_err(|_| {
        invalid_custody_composition_config("carrier peer DID is not a valid canonical did:key")
    })?;
    let canonical = crate::crypto::encode_did_key(&verifying_key).map_err(|_| {
        invalid_custody_composition_config("carrier peer DID cannot be canonicalized")
    })?;
    if canonical != peer_did {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config("carrier peer DID is not canonical")
        );
    }
    Ok(canonical)
}

fn load_runtime_custody_composition_config(
    data_dir: &Path,
) -> anyhow::Result<Option<RuntimeValidatedCustodyCompositionConfig>> {
    let config_path = runtime_custody_composition_config_path(data_dir);
    match fs::symlink_metadata(&config_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            anyhow::bail!(
                "{}",
                invalid_custody_composition_config("custody-composition file is unavailable")
            )
        }
    }
    let _protected_root = validate_owner_only_protected_content_dir(data_dir)?;
    let bytes = read_owner_only_config_bytes(&config_path, MAX_CUSTODY_COMPOSITION_BYTES)?;
    let config: RuntimeCustodyCompositionConfigFile =
        serde_json::from_slice(&bytes).map_err(|_| {
            invalid_custody_composition_config(
                "custody-composition file is not valid canonical JSON",
            )
        })?;
    if config.schema != CUSTODY_COMPOSITION_SCHEMA_V1
        || canonical_custody_composition_config_bytes(&config)? != bytes
    {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config("custody-composition file is not canonical")
        );
    }

    let expected_policy_authority = decode_custody_epoch_issuer_base64(
        &config.expected_policy_authority_base64,
        "expected_policy_authority",
    )?;
    let expected_authorization_identity = decode_canonical_contract_base64(
        &config.expected_committee_authorization_identity_base64,
        MAX_CUSTODY_COMPOSITION_BLOB_BYTES,
        "expected_committee_authorization_identity",
    )?;
    let signed_pool = decode_canonical_contract_base64(
        &config.signed_pool_base64,
        MAX_CUSTODY_COMPOSITION_BLOB_BYTES,
        "signed_pool",
    )?;
    let signed_epoch = decode_canonical_contract_base64(
        &config.signed_epoch_base64,
        MAX_CUSTODY_COMPOSITION_BLOB_BYTES,
        "signed_epoch",
    )?;
    let signed_committee_authorization = decode_canonical_contract_base64(
        &config.signed_committee_authorization_base64,
        MAX_CUSTODY_COMPOSITION_BLOB_BYTES,
        "signed_committee_authorization",
    )?;

    let now = crate::auth::now_ts();
    let validated = validate_custody_epoch_against_pool_at(
        expected_policy_authority,
        expected_authorization_identity,
        &signed_pool,
        &signed_epoch,
        &signed_committee_authorization,
        now,
    )
    .map_err(|_| {
        invalid_custody_composition_config(
            "signed pool/epoch/committee authorization does not validate against trust anchors",
        )
    })?;
    let committee_nodes = validated.committee().nodes();
    if committee_nodes.len() != 3 || config.routes.len() != 3 {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config(
                "custody-composition routes must cover exactly three selected nodes"
            )
        );
    }

    let mut configured_node_keys = std::collections::BTreeSet::new();
    let mut configured_roots = std::collections::BTreeSet::new();
    let mut configured_peer_dids = std::collections::BTreeSet::new();
    let mut local_count = 0usize;
    let mut route_bindings =
        std::collections::BTreeMap::<NodePublicKey, (Digest32, ProviderInvocationTransport)>::new();

    for route in &config.routes {
        let node_public_key =
            decode_node_public_key_base64(&route.node_public_key_base64, "route.node_public_key")?;
        let owner_state_root =
            decode_digest32_base64(&route.owner_state_root_base64, "route.owner_state_root")?;
        if owner_state_root == Digest32::new([0; 32])
            || !configured_node_keys.insert(node_public_key)
            || !configured_roots.insert(owner_state_root)
        {
            anyhow::bail!(
                "{}",
                invalid_custody_composition_config(
                    "custody-composition routes are duplicated or invalid"
                )
            );
        }
        let transport = match &route.transport {
            RuntimeCustodyRouteTransportConfig::Local => {
                local_count += 1;
                ProviderInvocationTransport::Local
            }
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid { peer_did } => {
                let canonical_peer_did = decode_canonical_peer_did(peer_did)?;
                if !configured_peer_dids.insert(canonical_peer_did.clone()) {
                    anyhow::bail!(
                        "{}",
                        invalid_custody_composition_config("carrier peer DIDs must be distinct")
                    );
                }
                ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                    peer_did: canonical_peer_did,
                    timeout_ms: None,
                })
            }
        };
        route_bindings.insert(node_public_key, (owner_state_root, transport));
    }

    if local_count > 1 {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config("at most one local custody route is allowed")
        );
    }

    let routes = committee_nodes
        .iter()
        .map(|node| {
            let (owner_state_root, transport) = route_bindings
                .remove(&node.node_public_key())
                .ok_or_else(|| {
                    invalid_custody_composition_config(
                        "custody-composition routes do not exactly cover the signed node set",
                    )
                })?;
            Ok(RuntimeValidatedCustodyRouteBinding {
                owner_state_root,
                transport,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if !route_bindings.is_empty() {
        anyhow::bail!(
            "{}",
            invalid_custody_composition_config(
                "custody-composition routes do not exactly cover the signed node set",
            )
        );
    }

    Ok(Some(RuntimeValidatedCustodyCompositionConfig {
        expected_policy_authority,
        expected_authorization_identity,
        signed_pool,
        signed_epoch,
        signed_committee_authorization,
        routes: routes.try_into().map_err(|_| {
            invalid_custody_composition_config(
                "custody-composition routes must cover exactly three selected nodes",
            )
        })?,
    }))
}

pub(crate) fn load_runtime_custody_composition(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
) -> anyhow::Result<Option<RuntimeCustodyComposition>> {
    let Some(config) = load_runtime_custody_composition_config(data_dir)? else {
        return Ok(None);
    };
    let now = crate::auth::now_ts();
    let validated = validate_custody_epoch_against_pool_at(
        config.expected_policy_authority,
        config.expected_authorization_identity,
        &config.signed_pool,
        &config.signed_epoch,
        &config.signed_committee_authorization,
        now,
    )
    .map_err(|_| {
        invalid_custody_composition_config(
            "signed pool/epoch/committee authorization does not validate against trust anchors",
        )
    })?;
    let nodes = validated
        .committee()
        .nodes()
        .iter()
        .zip(config.routes.iter())
        .map(|(node, route)| RuntimeCustodyCompositionNode {
            node_public_key: node.node_public_key(),
            custody_public_key: node.custody_public_key(),
            owner_state_root: route.owner_state_root,
            adapter: RuntimeCustodyRegistryAdapter::new(registry.clone(), route.transport.clone()),
        })
        .collect::<Vec<_>>();

    let composition = RuntimeCustodyComposition {
        expected_policy_authority: config.expected_policy_authority,
        expected_authorization_identity: config.expected_authorization_identity,
        signed_pool: config.signed_pool,
        signed_epoch: config.signed_epoch,
        signed_committee_authorization: config.signed_committee_authorization,
        nodes: nodes.try_into().map_err(|_| {
            invalid_custody_composition_config(
                "custody-composition routes must cover exactly three selected nodes",
            )
        })?,
    };
    let configured = composition
        .configured_nodes()
        .map_err(|_| invalid_custody_composition_config("configured custody linkage is invalid"))?;
    resolve_runtime_mint_selected_nodes(
        composition.expected_policy_authority,
        composition.expected_authorization_identity,
        &composition.signed_pool,
        &composition.signed_epoch,
        &composition.signed_committee_authorization,
        now,
        &configured,
    )
    .map_err(|_| invalid_custody_composition_config("configured custody linkage is invalid"))?;
    Ok(Some(composition))
}

pub fn log_unresolved_runtime_releases(data_dir: &Path) {
    match list_unresolved_runtime_releases(data_dir) {
        Ok(operations) => {
            let effect_started = operations
                .iter()
                .filter(|operation| operation.provider_effect_started())
                .count();
            if operations.is_empty() {
                return;
            }
            tracing::warn!(
                unresolved = operations.len(),
                effect_started,
                "protected-content runtime release journal has unresolved operations; Runtime will not infer completion from absence or elapsed time"
            );
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "protected-content runtime release journal scan failed closed"
            );
        }
    }
}

pub(crate) async fn invoke_json_provider(
    registry: &ProviderRegistry,
    target: &str,
    op: &str,
    request: Value,
) -> Result<Value, String> {
    invoke_json_provider_with_transport(
        registry,
        target,
        op,
        request,
        ProviderInvocationTransport::Local,
    )
    .await
}

pub(crate) async fn invoke_json_provider_with_transport(
    registry: &ProviderRegistry,
    target: &str,
    op: &str,
    request: Value,
    transport: ProviderInvocationTransport,
) -> Result<Value, String> {
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: RUNTIME_PROVIDER_ID.to_string(),
            target: target.to_string(),
            op: op.to_string(),
            request,
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport,
        })
        .await
        .map_err(|_| format!("{target} provider {op} invocation failed"))?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{target} provider {op} response is missing status"))?;
    match status {
        "ok" => response
            .get("data")
            .cloned()
            .ok_or_else(|| format!("{target} provider {op} response is missing data")),
        "error" => Err(format!("{target} provider {op} rejected the request")),
        _ => Err(format!(
            "{target} provider {op} response has unsupported status"
        )),
    }
}

/// Publishes and verifies the one immutable CENC/fMP4 object Runtime may later
/// record as availability evidence. The directory is provider input only: no
/// file path, bytes, or receipt JSON crosses this boundary into Runtime state.
pub async fn publish_and_verify_protected_content_availability(
    registry: &ProviderRegistry,
    protected_content_dir: &Path,
    media_identity: &CencFmp4MediaIdentityV1,
    requirement: &RuntimeContentAvailabilityRequirement,
    now_unix_seconds: u64,
) -> anyhow::Result<RuntimeVerifiedContentAvailability> {
    verify_protected_content_directory(protected_content_dir, media_identity)?;
    let content_cid = crate::content::publish_directory_via_provider_with_kind(
        registry,
        protected_content_dir,
        PROTECTED_CONTENT_OBJECT_KIND,
        Some(requirement.expected_object_identity()),
        Some(requirement.expected_publisher_did()),
    )
    .await?;
    let receipt = fetch_content_availability_receipt(registry, &content_cid).await?;
    let manifest = crate::content::fetch_content_object_manifest(registry, &content_cid).await?;
    verify_protected_content_manifest_and_files(registry, &content_cid, &manifest, media_identity)
        .await?;
    verify_protected_content_receipt(
        &content_cid,
        &manifest,
        &receipt,
        media_identity,
        requirement,
        now_unix_seconds,
    )
}

async fn fetch_content_availability_receipt(
    registry: &ProviderRegistry,
    content_cid: &str,
) -> anyhow::Result<Vec<u8>> {
    let response = registry
        .send_raw(
            CONTENT_PROVIDER_ID,
            &json!({ "op": "status", "cid": content_cid }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("protected content status is unavailable"))?;
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        anyhow::bail!("protected content status is invalid");
    }
    let receipt = response
        .get("data")
        .and_then(|data| data.get("receipt"))
        .ok_or_else(|| anyhow::anyhow!("protected content status receipt is absent"))?;
    serde_json::to_vec(receipt)
        .map_err(|_| anyhow::anyhow!("protected content status receipt is invalid"))
}

fn verify_protected_content_directory(
    directory: &Path,
    media_identity: &CencFmp4MediaIdentityV1,
) -> anyhow::Result<()> {
    let expected_files = protected_content_files(media_identity)?;
    let expected_paths: Vec<String> = expected_files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    let actual_paths = protected_content_directory_files(directory)?;
    if actual_paths != expected_paths {
        anyhow::bail!("protected content directory layout is invalid");
    }
    let descriptor = fs::read(directory.join(PROTECTED_CONTENT_IDENTITY_PATH))
        .map_err(|_| anyhow::anyhow!("protected content descriptor is unavailable"))?;
    let canonical_identity = media_identity
        .canonical_bytes()
        .map_err(|_| anyhow::anyhow!("protected content media identity is invalid"))?;
    if descriptor != canonical_identity
        || CencFmp4MediaIdentityV1::from_canonical_bytes(&descriptor)
            .map_err(|_| anyhow::anyhow!("protected content descriptor is invalid"))?
            != *media_identity
    {
        anyhow::bail!("protected content descriptor is invalid");
    }
    let init = fs::read(directory.join(PROTECTED_CONTENT_INIT_PATH))
        .map_err(|_| anyhow::anyhow!("protected content init is unavailable"))?;
    let mut segments = Vec::with_capacity(media_identity.encrypted_segments().len());
    for index in 0..media_identity.encrypted_segments().len() {
        segments.push(
            fs::read(directory.join(protected_content_segment_path(index)))
                .map_err(|_| anyhow::anyhow!("protected content segment is unavailable"))?,
        );
    }
    let reconstructed = CencFmp4MediaIdentityV1::new_from_bytes(
        &init,
        &segments,
        media_identity.mime_type(),
        media_identity.codecs(),
    )
    .map_err(|_| anyhow::anyhow!("protected content media layout is invalid"))?;
    if reconstructed != *media_identity {
        anyhow::bail!("protected content media identity is invalid");
    }
    Ok(())
}

fn protected_content_directory_files(directory: &Path) -> anyhow::Result<Vec<String>> {
    let mut actual_paths = Vec::new();
    collect_protected_content_directory_files(directory, directory, &mut actual_paths)?;
    actual_paths.sort_unstable();
    Ok(actual_paths)
}

fn collect_protected_content_directory_files(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(directory)
        .map_err(|_| anyhow::anyhow!("protected content directory is unavailable"))?
    {
        let entry = entry.map_err(|_| anyhow::anyhow!("protected content directory is invalid"))?;
        let file_type = entry
            .file_type()
            .map_err(|_| anyhow::anyhow!("protected content directory is invalid"))?;
        if file_type.is_symlink() {
            anyhow::bail!("protected content directory is invalid");
        }
        if file_type.is_dir() {
            collect_protected_content_directory_files(root, &entry.path(), paths)?;
            continue;
        }
        if !file_type.is_file() {
            anyhow::bail!("protected content directory is invalid");
        }
        let entry_path = entry.path();
        let relative = entry_path
            .strip_prefix(root)
            .map_err(|_| anyhow::anyhow!("protected content directory is invalid"))?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("protected content directory is invalid"))?;
        let normalized = relative.replace('\\', "/");
        match normalized.as_str() {
            PROTECTED_CONTENT_IDENTITY_PATH | PROTECTED_CONTENT_INIT_PATH => {}
            _ if normalized.starts_with(PROTECTED_CONTENT_SEGMENTS_PREFIX)
                && normalized.ends_with(PROTECTED_CONTENT_SEGMENTS_SUFFIX) =>
            {
                let index = normalized
                    .strip_prefix(PROTECTED_CONTENT_SEGMENTS_PREFIX)
                    .and_then(|value| value.strip_suffix(PROTECTED_CONTENT_SEGMENTS_SUFFIX))
                    .and_then(|value| value.parse::<usize>().ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!("protected content directory layout is invalid")
                    })?;
                if normalized != protected_content_segment_path(index) {
                    anyhow::bail!("protected content directory layout is invalid");
                }
            }
            _ => anyhow::bail!("protected content directory layout is invalid"),
        }
        paths.push(normalized);
    }
    Ok(())
}

fn protected_content_files(
    media_identity: &CencFmp4MediaIdentityV1,
) -> anyhow::Result<Vec<crate::content::ContentObjectFile>> {
    let descriptor = media_identity
        .canonical_bytes()
        .map_err(|_| anyhow::anyhow!("protected content media identity is invalid"))?;
    let mut files = Vec::with_capacity(media_identity.encrypted_segments().len() + 2);
    files.push(content_object_file(
        PROTECTED_CONTENT_IDENTITY_PATH,
        &descriptor,
    ));
    files.push(crate::content::ContentObjectFile {
        path: PROTECTED_CONTENT_INIT_PATH.to_string(),
        sha256: hex::encode(media_identity.init_segment_sha256().as_bytes()),
        size: media_identity.init_segment_bytes(),
    });
    files.extend(
        media_identity
            .encrypted_segments()
            .iter()
            .enumerate()
            .map(|(index, segment)| crate::content::ContentObjectFile {
                path: protected_content_segment_path(index),
                sha256: hex::encode(segment.ciphertext_sha256().as_bytes()),
                size: segment.ciphertext_bytes(),
            }),
    );
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn content_object_file(path: &str, bytes: &[u8]) -> crate::content::ContentObjectFile {
    crate::content::ContentObjectFile {
        path: path.to_string(),
        sha256: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

fn protected_content_segment_path(index: usize) -> String {
    format!("{PROTECTED_CONTENT_SEGMENTS_PREFIX}{index:08}{PROTECTED_CONTENT_SEGMENTS_SUFFIX}")
}

async fn verify_protected_content_manifest_and_files(
    registry: &ProviderRegistry,
    content_cid: &str,
    manifest: &crate::content::ContentObjectManifest,
    media_identity: &CencFmp4MediaIdentityV1,
) -> anyhow::Result<()> {
    let expected_files = protected_content_files(media_identity)?;
    if manifest.schema != "elastos.content.object.manifest/v1"
        || manifest.kind != PROTECTED_CONTENT_OBJECT_KIND
        || !manifest.links.is_empty()
        || !content_object_files_match(&manifest.files, &expected_files)
        || manifest.content_digest != content_object_digest(&expected_files)
    {
        anyhow::bail!("protected content object manifest is invalid");
    }
    let mut protected_init = None;
    let mut encrypted_segments = Vec::with_capacity(media_identity.encrypted_segments().len());
    for file in &expected_files {
        let bytes =
            crate::content::fetch_bytes_via_provider(registry, content_cid, Some(&file.path))
                .await?;
        crate::content::verify_content_object_file(content_cid, file, &bytes)?;
        match file.path.as_str() {
            PROTECTED_CONTENT_IDENTITY_PATH => {
                let canonical_identity = media_identity
                    .canonical_bytes()
                    .map_err(|_| anyhow::anyhow!("protected content media identity is invalid"))?;
                if bytes != canonical_identity
                    || CencFmp4MediaIdentityV1::from_canonical_bytes(&bytes)
                        .map_err(|_| anyhow::anyhow!("protected content descriptor is invalid"))?
                        != *media_identity
                {
                    anyhow::bail!("protected content descriptor is invalid");
                }
            }
            PROTECTED_CONTENT_INIT_PATH => protected_init = Some(bytes),
            _ => encrypted_segments.push(bytes),
        }
    }
    let protected_init =
        protected_init.ok_or_else(|| anyhow::anyhow!("protected content init is absent"))?;
    let reconstructed = CencFmp4MediaIdentityV1::new_from_bytes(
        &protected_init,
        &encrypted_segments,
        media_identity.mime_type(),
        media_identity.codecs(),
    )
    .map_err(|_| anyhow::anyhow!("protected content media layout is invalid"))?;
    if reconstructed != *media_identity {
        anyhow::bail!("protected content media identity is invalid");
    }
    Ok(())
}

fn content_object_files_match(
    actual: &[crate::content::ContentObjectFile],
    expected: &[crate::content::ContentObjectFile],
) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.path == expected.path
                && actual.sha256 == expected.sha256
                && actual.size == expected.size
        })
}

fn content_object_digest(files: &[crate::content::ContentObjectFile]) -> String {
    let mut hasher = sha2::Sha256::new();
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.size.to_string().as_bytes());
        hasher.update(b"\0");
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn verify_protected_content_receipt(
    content_cid: &str,
    manifest: &crate::content::ContentObjectManifest,
    receipt_json: &[u8],
    media_identity: &CencFmp4MediaIdentityV1,
    requirement: &RuntimeContentAvailabilityRequirement,
    now_unix_seconds: u64,
) -> anyhow::Result<RuntimeVerifiedContentAvailability> {
    cid::Cid::try_from(content_cid)
        .map_err(|_| anyhow::anyhow!("protected content availability CID is invalid"))?;
    let receipt: crate::content::SignedAvailabilityReceipt =
        serde_json::from_slice(receipt_json)
            .map_err(|_| anyhow::anyhow!("protected content availability receipt is invalid"))?;
    let receipt_json = serde_json::to_vec(&receipt)
        .map_err(|_| anyhow::anyhow!("protected content availability receipt is invalid"))?;
    if receipt.payload.schema != CONTENT_AVAILABILITY_RECEIPT_SCHEMA
        || receipt.signer_did != requirement.expected_provider_did()
        || receipt.payload.cid != content_cid
        || receipt.payload.uri != format!("elastos://{content_cid}")
        || receipt.payload.object_did.as_deref() != Some(requirement.expected_object_identity())
        || receipt.payload.publisher_did != requirement.expected_publisher_did()
        || receipt.payload.policy != requirement.policy()
        || receipt.payload.status != PROTECTED_CONTENT_AVAILABLE_STATUS
        || receipt.payload.replicas < requirement.minimum_replicas()
        || manifest.object_did.as_deref() != Some(requirement.expected_object_identity())
        || manifest.publisher_did.as_deref() != Some(requirement.expected_publisher_did())
    {
        anyhow::bail!("protected content availability receipt binding is invalid");
    }
    let max_checked_at = now_unix_seconds
        .checked_add(requirement.max_future_skew_seconds())
        .ok_or_else(|| anyhow::anyhow!("protected content availability time is invalid"))?;
    if receipt.payload.checked_at > max_checked_at
        || now_unix_seconds.saturating_sub(receipt.payload.checked_at)
            > requirement.max_age_seconds()
    {
        anyhow::bail!("protected content availability receipt is outside its freshness window");
    }
    crate::crypto::verify_signed_json_envelope_against_dids(
        &receipt_json,
        CONTENT_AVAILABILITY_RECEIPT_DOMAIN,
        std::slice::from_ref(&requirement.expected_provider_did().to_string()),
    )
    .map_err(|_| anyhow::anyhow!("protected content availability receipt signature is invalid"))?;
    RuntimeVerifiedContentAvailability::new(
        content_cid,
        requirement.expected_object_identity(),
        requirement.expected_publisher_did(),
        requirement,
        receipt.payload.replicas,
        receipt.payload.checked_at,
        Digest32::new(sha2::Sha256::digest(&receipt_json).into()),
        media_identity.encrypted_content().clone(),
        media_identity.media_manifest_root(),
    )
    .map_err(|_| anyhow::anyhow!("protected content availability evidence is invalid"))
}
