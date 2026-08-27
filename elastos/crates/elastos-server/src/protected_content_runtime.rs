//! Protected-content Runtime provider plane.
//!
//! This module registers Runtime-owned `protect` and `custody` routes, scans
//! the durable Runtime release journal on startup, and evaluates policy through
//! the existing `chain` registry route. Library mint and buy use these seams.
//! It does not replace the provisional `key`/`rights`/`drm`/`decrypt`
//! open/share path or expose provider topology to capsules.

#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Write as _};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};

use base64::Engine as _;
use ed25519_dalek::Signer as _;
use elastos_protected_content_contracts::{
    validate_custody_epoch_against_pool_at, CanonicalContract, ContentAccessIdV1,
    CustodyCommitteeAuthorizationIdentityV1, CustodyEnvelopeV1, CustodyEpochIssuerKeyV1, Digest32,
    EncryptedContentIdentityV1, KeyReleaseOutcomeV1, KeyReleaseRequestV1, NodeContributionRefV1,
    NodeCustodyPublicKeyV1, NodePublicKey, ProfileIdentityV1, RecipientKeyAuthorizationStatementV1,
    RecipientKeyIdentityV1, RecipientPublicKeyBytesV1, ReplayNonce16, RightsActionV1,
    RightsEvaluationEvidenceRequestV1, RightsPolicyBodyV1, RightsPolicyIdentityV1, RightsRequestV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
    RuntimeSessionBindingV1, SignedCustodyCommitteeAuthorizationV1, SignedCustodyEpochV1,
    SignedCustodyPoolV1, SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1,
    TerminalReceiptIssuerKey, TerminalReceiptStatementV1, WalletSignedRightsRequestV1,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, DecryptProviderRequestOpV1, DecryptProviderRequestV1,
    DecryptProviderResponseV1, ProtectProviderRequestV1, ProtectProviderResponseStatusV1,
    ProtectProviderResponseV1, ProtectionSessionNodeV1, RightsProviderRequestV1,
    RightsProviderResponseV1, ValidatedCencFmp4MediaSessionLayoutV1, ViewerMediaPartSelectorV1,
    MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
};
use elastos_protected_content_rights::{
    PrivateCustodyRightsRequestV1, CHAIN_PROVIDER_ID, CHAIN_RIGHTS_EVIDENCE_OP,
};
use elastos_protected_content_runtime::RuntimeProviderCallError;
use elastos_protected_content_runtime::{
    bind_buy, cancel_prepared_recipient, cancel_prepared_recipient_with_result_by_handle,
    close_viewer_session_with_result, open_viewer_session, prepare_recipient,
    read_viewer_media_part, resolve_runtime_mint_selected_nodes, PersistedRuntimeMint,
    PersistedRuntimeReleaseOperation, RuntimeContentAvailabilityRequirement,
    RuntimeDecryptProvider, RuntimeMintConfiguredCustodyProvider, RuntimeMintCoordinator,
    RuntimeMintCoordinatorError, RuntimeMintCoordinatorOutcome, RuntimeMintCreatorTerminalEvidence,
    RuntimeMintDraft, RuntimeMintIntent, RuntimeMintJournal, RuntimeOpenViewerSessionInput,
    RuntimePreparedRecipientCancelResult, RuntimeProtectedContentPurchaseIntent,
    RuntimePurchaseEffectAuthority, RuntimeReleaseAuditRecord, RuntimeReleaseCoordinator,
    RuntimeReleaseCoordinatorOutcome, RuntimeReleaseJournal, RuntimeReleaseJournalError,
    RuntimeReleaseTerminalResult, RuntimeSelectedProvider, RuntimeVerifiedContentAvailability,
    RuntimeVerifiedPurchaseEffect, RuntimeViewerSession, RuntimeViewerSessionCloseResult,
};
use elastos_runtime::provider::bridge::{ProviderBridge, ProviderConfig};
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer, ResourceRequest, ResourceResponse,
};
use elastos_wallet_contract::{
    ValidatedChainOutcomeBindingV1, VerifiedWalletInvocationContext, WalletProviderOperationV2,
    WalletProviderRequestV2, MAX_INVOCATION_TTL_SECS, WALLET_BUS_OPERATION,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest as _;

pub(crate) const CUSTODY_PROVIDER_ID: &str = "custody";
pub(crate) const PROTECT_PROVIDER_ID: &str = "protect";
pub(crate) const RUNTIME_PROVIDER_ID: &str = "runtime";
const CONTENT_PROVIDER_ID: &str = "content";
const PROTECTED_CONTENT_REPLICATION_POLICY: &str = "protected-content-replication/v1";
const PROTECTED_CONTENT_MIN_REPLICAS: u32 = 3;
const PROTECTED_CONTENT_AVAILABILITY_MAX_AGE_SECS: u64 = 60;
const PROTECTED_CONTENT_AVAILABILITY_MAX_FUTURE_SKEW_SECS: u64 = 5;
pub(crate) const RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE: &str =
    "Runtime custody composition is not configured";
pub(crate) const RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE: &str =
    "Runtime custody purchase is denied before buy";
pub(crate) const RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE: &str =
    "Runtime custody purchase is pending exact Wallet or Chain settlement";
pub(crate) const RUNTIME_CUSTODY_PURCHASE_UNAVAILABLE_MESSAGE: &str =
    "Runtime custody purchase is unavailable";
pub(crate) const RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE: &str =
    "Runtime custody open is denied before purchase";
pub(crate) const RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE: &str =
    "Runtime custody decrypt provider is unavailable";
pub(crate) const RUNTIME_CUSTODY_VIEWER_ENVELOPE_UNAVAILABLE_MESSAGE: &str =
    "Runtime custody viewer envelope is unavailable";
pub(crate) const RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE: &str =
    "Runtime custody viewer release approval is unavailable";
pub(crate) const RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE: &str =
    "Runtime custody mint requires cleanup reconciliation";
pub(crate) const RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE: &str =
    "Runtime custody mint was settled before draft persistence";
const PROTECTED_CONTENT_ROOT: &str = "protected-content";
const CUSTODY_COMPOSITION_CONFIG_FILE: &str = "protected-content/custody-composition.json";
const RUNTIME_MINT_JOURNAL_ROOT: &str = "protected-content/runtime-mint";
const RUNTIME_OPEN_MATERIAL_ROOT: &str = "protected-content/runtime-open";
const RUNTIME_LISTING_ROOT: &str = "protected-content/runtime-listings";
const RUNTIME_PURCHASE_ROOT: &str = "protected-content/runtime-purchases";
const RUNTIME_VIEWER_SCHEMA_V1: &str = "elastos.library.runtime-custody-viewer-state/v1";
const RUNTIME_LISTING_SCHEMA_V1: &str = "elastos.library.runtime-custody-listing/v1";
pub(crate) const RUNTIME_PURCHASE_SCHEMA_V1: &str = "elastos.library.runtime-custody-purchase/v1";
static RUNTIME_VIEWER_LIFECYCLE_GUARDS: OnceLock<
    StdMutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
> = OnceLock::new();
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
const MAX_RUNTIME_VIEWER_RECONCILE_MINT_DIRS: usize = 256;
const MAX_RUNTIME_VIEWER_RECONCILE_RECORDS: usize = 1024;
const MAX_RUNTIME_VIEWER_RECORD_BYTES: u64 = 64 * 1024;
const DECRYPT_PROVIDER_ID: &str = "decrypt";
const WALLET_PROVIDER_ID: &str = "wallet";
const INACTIVE_CUSTODY_ROOT: &str = "protected-content/custody-provider/inactive";
const PROVIDER_INVOCATION_SCHEMA_V1: &str = "elastos.provider.invocation/v1";
const CHAIN_PROTECTED_CONTENT_POLICY_OP: &str = "resolve_protected_content_policy";
const CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1: &str = "elastos.chain.protected-content-policy/v1";

pub(crate) struct ResolvedRuntimeRightsPolicy {
    body: RightsPolicyBodyV1,
    identity: RightsPolicyIdentityV1,
}

impl ResolvedRuntimeRightsPolicy {
    pub(crate) fn body(&self) -> &RightsPolicyBodyV1 {
        &self.body
    }

    pub(crate) fn identity(&self) -> &RightsPolicyIdentityV1 {
        &self.identity
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChainProtectedContentPolicyResponse {
    schema: String,
    policy_body: String,
}

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

struct RuntimeDecryptRegistryAdapter {
    registry: Arc<ProviderRegistry>,
}

impl RuntimeDecryptRegistryAdapter {
    fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self { registry }
    }
}

fn decrypt_provider_op(request: &DecryptProviderRequestV1) -> &'static str {
    match request.op() {
        DecryptProviderRequestOpV1::PrepareRecipient => "prepare_recipient",
        DecryptProviderRequestOpV1::OpenViewerSession => "open_viewer_session",
        DecryptProviderRequestOpV1::ReadViewerMediaPart => "read_viewer_media_part",
        DecryptProviderRequestOpV1::CancelPreparedRecipient => "cancel_prepared_recipient",
        DecryptProviderRequestOpV1::CloseViewerSession => "close_viewer_session",
    }
}

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

fn generate_runtime_content_access_id() -> anyhow::Result<ContentAccessIdV1> {
    loop {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        if let Ok(value) = ContentAccessIdV1::new(bytes) {
            return Ok(value);
        }
    }
}

fn runtime_mint_intent_with_access_id(
    composition: &RuntimeCustodyComposition,
    input: &RuntimeCustodyLibraryPublishInput,
    selected_nodes: Vec<elastos_protected_content_runtime::RuntimeMintNodeBinding>,
    content_access_id: ContentAccessIdV1,
) -> anyhow::Result<RuntimeMintIntent> {
    RuntimeMintIntent::new(
        input.principal_id.clone(),
        &input.object_uri,
        &input.source_storage,
        input.mime_type.clone(),
        input.codecs.clone(),
        &input.clear_init_segment,
        &input.clear_segments,
        content_access_id,
        composition
            .signed_pool
            .pool_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is invalid"))?,
        composition
            .signed_epoch
            .epoch_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is invalid"))?,
        composition
            .signed_committee_authorization
            .authorization_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is invalid"))?,
        selected_nodes,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is invalid"))
}

fn load_or_persist_runtime_mint_intent(
    journal: &RuntimeMintJournal,
    composition: &RuntimeCustodyComposition,
    input: &RuntimeCustodyLibraryPublishInput,
    selected_nodes: Vec<elastos_protected_content_runtime::RuntimeMintNodeBinding>,
) -> anyhow::Result<RuntimeMintIntent> {
    let request_id = RuntimeMintIntent::request_id_for_source(
        &input.principal_id,
        &input.object_uri,
        &input.source_storage,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is invalid"))?;
    match journal.load_intent(request_id) {
        Ok(existing) => {
            let expected = runtime_mint_intent_with_access_id(
                composition,
                input,
                selected_nodes,
                existing.content_access_id(),
            )?;
            if !existing.same_authority_as(&expected) {
                anyhow::bail!("Runtime custody mint intent conflicts with existing authority");
            }
            Ok(existing)
        }
        Err(elastos_protected_content_runtime::RuntimeMintJournalError::NotFound) => {
            let intent = runtime_mint_intent_with_access_id(
                composition,
                input,
                selected_nodes,
                generate_runtime_content_access_id()?,
            )?;
            journal
                .persist_intent(&intent)
                .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))
        }
        Err(_) => Err(anyhow::anyhow!(
            "Runtime custody mint intent is unavailable"
        )),
    }
}

fn load_completed_runtime_mint_facts(
    journal: &RuntimeMintJournal,
    input: &RuntimeCustodyLibraryPublishInput,
    mint_id: Digest32,
) -> anyhow::Result<RuntimeCustodyLibraryPublishFacts> {
    let persisted = journal
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
    let evidence = persisted
        .content_availability()
        .ok_or_else(|| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
    let content_id = runtime_protected_content_id(persisted.draft().encrypted_content())?;
    Ok(runtime_custody_library_publish_facts(
        input,
        persisted.draft(),
        &content_id,
        evidence,
    ))
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

pub async fn register_protect_provider(
    registry: &Arc<ProviderRegistry>,
    binary_path: &Path,
) -> anyhow::Result<()> {
    let bridge = elastos_runtime::provider::ProviderBridge::spawn(
        binary_path,
        ProviderConfig {
            extra: json!({}),
            ..Default::default()
        },
    )
    .await
    .map_err(|error| anyhow::anyhow!("failed to spawn protect provider: {error}"))?;
    let provider: Arc<dyn Provider> =
        Arc::new(elastos_runtime::provider::CapsuleProvider::with_scheme(
            Arc::new(bridge),
            PROTECT_PROVIDER_ID,
        ));
    registry.register(provider).await;
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
    std::io::Read::by_ref(&mut file)
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

pub(crate) fn runtime_protected_content_id(
    encrypted_content: &EncryptedContentIdentityV1,
) -> anyhow::Result<String> {
    Ok(format!(
        "content:{}",
        hex::encode(encrypted_content.canonical_hash()?.as_bytes())
    ))
}

fn runtime_protected_content_identity_hex(
    encrypted_content: &EncryptedContentIdentityV1,
) -> anyhow::Result<String> {
    Ok(format!(
        "0x{}",
        hex::encode(
            encrypted_content
                .canonical_bytes()
                .map_err(|_| anyhow::anyhow!("protected content identity is invalid"))?
        )
    ))
}

fn runtime_content_access_id_hex(content_access_id: ContentAccessIdV1) -> String {
    format!("0x{}", hex::encode(content_access_id.as_bytes()))
}

pub(crate) async fn resolve_runtime_rights_policy(
    registry: &ProviderRegistry,
    encrypted_content: &EncryptedContentIdentityV1,
    content_access_id: ContentAccessIdV1,
    action: RightsActionV1,
) -> anyhow::Result<ResolvedRuntimeRightsPolicy> {
    let response_value = invoke_json_provider(
        registry,
        CHAIN_PROVIDER_ID,
        CHAIN_PROTECTED_CONTENT_POLICY_OP,
        json!({
            "op": CHAIN_PROTECTED_CONTENT_POLICY_OP,
            "encrypted_content": runtime_protected_content_identity_hex(encrypted_content)?,
            "content_access_id": runtime_content_access_id_hex(content_access_id),
            "action": runtime_rights_action_name(action),
        }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    let response = serde_json::from_value::<ChainProtectedContentPolicyResponse>(response_value)?;
    if response.schema != CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1 {
        anyhow::bail!("chain provider returned an unsupported protected-content policy schema");
    }
    let policy_bytes = decode_0x_hex(&response.policy_body).ok_or_else(|| {
        anyhow::anyhow!("chain provider returned an invalid protected-content policy body")
    })?;
    let body = RightsPolicyBodyV1::from_canonical_bytes(&policy_bytes)?;
    if body.encrypted_content() != encrypted_content
        || body.content_access_id() != content_access_id
        || body.required_action() != action
    {
        anyhow::bail!(
            "chain provider returned a protected-content policy that does not match the Runtime request"
        );
    }
    let identity = body.policy_identity()?;
    Ok(ResolvedRuntimeRightsPolicy { body, identity })
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

fn runtime_rights_action_name(action: RightsActionV1) -> &'static str {
    match action {
        RightsActionV1::View => "view",
        RightsActionV1::Stream => "stream",
        RightsActionV1::Download => "download",
        RightsActionV1::Execute => "execute",
    }
}

fn decode_0x_hex(value: &str) -> Option<Vec<u8>> {
    let hex = value.strip_prefix("0x")?;
    hex::decode(hex).ok()
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

#[derive(Clone)]
pub(crate) struct RuntimeCustodyLibraryPublishInput {
    pub object_uri: String,
    pub principal_id: String,
    pub mime_type: String,
    pub codecs: String,
    pub wallet_account_id: String,
    pub copies: String,
    pub price: String,
    pub clear_init_segment: Vec<u8>,
    pub clear_segments: Vec<Vec<u8>>,
    pub source_storage: String,
}

impl std::fmt::Debug for RuntimeCustodyLibraryPublishInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeCustodyLibraryPublishInput")
            .field("object_uri", &self.object_uri)
            .field("principal_id", &self.principal_id)
            .field("mime_type", &self.mime_type)
            .field("codecs", &self.codecs)
            .field("wallet_account_id", &self.wallet_account_id)
            .field("copies", &self.copies)
            .field("price", &self.price)
            .field("clear_init_segment_bytes", &self.clear_init_segment.len())
            .field("clear_segment_count", &self.clear_segments.len())
            .field("source_storage", &self.source_storage)
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeCustodyLibraryPublishFacts {
    pub content_cid: String,
    pub mint_id: Digest32,
    pub content_id: String,
    pub availability: Value,
    pub receipt: Value,
    pub content_security: Value,
}

pub(crate) async fn publish_runtime_custody_library_object(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    input: RuntimeCustodyLibraryPublishInput,
) -> anyhow::Result<RuntimeCustodyLibraryPublishFacts> {
    let composition = load_runtime_custody_composition(data_dir, registry.clone())?
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE))?;
    let (device_key, device_did) =
        crate::collaboration_profile_authority::load_existing_device_signing_key(data_dir)?
            .ok_or_else(|| anyhow::anyhow!("local Runtime device signing key is missing"))?;
    let runtime_issuer = RuntimeOperationIssuerKeyV1::new(device_key.verifying_key().to_bytes())
        .map_err(|_| anyhow::anyhow!("local Runtime device signing key is invalid"))?;
    let now = crate::auth::now_ts();
    let configured = composition
        .configured_nodes()
        .map_err(|_| anyhow::anyhow!("Runtime custody mint selection is invalid"))?;
    let selected = resolve_runtime_mint_selected_nodes(
        composition.expected_policy_authority,
        composition.expected_authorization_identity,
        &composition.signed_pool,
        &composition.signed_epoch,
        &composition.signed_committee_authorization,
        now,
        &configured,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody mint selection is invalid"))?;
    let mint_nodes = selected
        .iter()
        .map(|node| node.binding().clone())
        .collect::<Vec<_>>();
    let mint_journal = runtime_mint_journal(data_dir);
    let mint_intent = load_or_persist_runtime_mint_intent(
        &mint_journal,
        &composition,
        &input,
        mint_nodes.clone(),
    )?;
    if let Some(mint_id) = mint_intent.completed_mint_id() {
        return load_completed_runtime_mint_facts(&mint_journal, &input, mint_id);
    }
    let protected =
        protect_runtime_custody_media(&registry, &mint_journal, &composition, &input, &mint_intent)
            .await?;
    let policy = resolve_runtime_rights_policy(
        registry.as_ref(),
        protected.media_identity.encrypted_content(),
        mint_intent.content_access_id(),
        RightsActionV1::View,
    )
    .await
    .map_err(|_| anyhow::anyhow!("Runtime custody rights policy is unavailable"))?;
    let mint_draft = RuntimeMintDraft::new(
        &protected.init_segment,
        &protected.encrypted_segments,
        input.mime_type.clone(),
        input.codecs.clone(),
        mint_intent.content_access_id(),
        protected
            .envelope
            .key_envelope_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?,
        policy.identity().clone(),
        protected.envelope.manifest().content_key_commitment(),
        protected.envelope.manifest().threshold(),
        mint_nodes,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody mint draft is invalid"))?;
    let sign_key = device_key.clone();
    let coordinator = RuntimeMintCoordinator::new(
        runtime_mint_journal(data_dir),
        runtime_issuer,
        move |bytes| sign_key.sign(bytes).to_bytes(),
        selected,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody mint coordinator is invalid"))?;
    match coordinator
        .provision(&mint_draft, &protected.envelope, now)
        .await
        .map_err(|_| anyhow::anyhow!("Runtime custody mint failed"))?
    {
        RuntimeMintCoordinatorOutcome::CustodyProvisioned { mint_id }
            if mint_id == mint_draft.mint_id() => {}
        RuntimeMintCoordinatorOutcome::ContentAvailable { mint_id }
            if mint_id == mint_draft.mint_id() => {}
        _ => anyhow::bail!("Runtime custody mint failed"),
    }
    let content_id = runtime_protected_content_id(mint_draft.encrypted_content())?;
    let requirement = RuntimeContentAvailabilityRequirement::new(
        device_did,
        content_id.clone(),
        input.principal_id.clone(),
        PROTECTED_CONTENT_REPLICATION_POLICY,
        PROTECTED_CONTENT_MIN_REPLICAS,
        PROTECTED_CONTENT_AVAILABILITY_MAX_AGE_SECS,
        PROTECTED_CONTENT_AVAILABILITY_MAX_FUTURE_SKEW_SECS,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody availability requirement is invalid"))?;
    let staging = write_protected_content_staging_directory(data_dir, &protected)?;
    let evidence = publish_and_verify_protected_content_availability(
        registry.as_ref(),
        staging.path(),
        mint_draft.media_identity(),
        &requirement,
        crate::auth::now_ts(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    match coordinator
        .record_content_availability(&mint_draft, &requirement, evidence.clone())
        .map_err(|_| anyhow::anyhow!("Runtime custody availability record failed"))?
    {
        RuntimeMintCoordinatorOutcome::ContentAvailable { mint_id }
            if mint_id == mint_draft.mint_id() => {}
        _ => anyhow::bail!("Runtime custody availability record failed"),
    }
    persist_runtime_open_envelope(data_dir, mint_draft.mint_id(), &protected.envelope)?;
    let facts = runtime_custody_library_publish_facts(&input, &mint_draft, &content_id, &evidence);
    mint_journal
        .mark_intent_completed(mint_intent.request_id(), mint_draft.mint_id())
        .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
    Ok(facts)
}

struct ProtectedRuntimeCustodyMedia {
    init_segment: Vec<u8>,
    encrypted_segments: Vec<Vec<u8>>,
    media_identity: CencFmp4MediaIdentityV1,
    envelope: CustodyEnvelopeV1,
}

enum RuntimeProtectRecoveryDisposition {
    Fresh,
    ReplayOpenAndSettleCancel,
    SettleCancel([u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]),
    SettleClose([u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]),
    TerminalAbort,
}

async fn protect_runtime_custody_media(
    registry: &ProviderRegistry,
    journal: &RuntimeMintJournal,
    composition: &RuntimeCustodyComposition,
    input: &RuntimeCustodyLibraryPublishInput,
    mint_intent: &RuntimeMintIntent,
) -> anyhow::Result<ProtectedRuntimeCustodyMedia> {
    let open_request =
        runtime_custody_open_protection_session_request(composition, input, mint_intent)?;
    match runtime_protect_recovery_disposition(mint_intent) {
        RuntimeProtectRecoveryDisposition::TerminalAbort => {
            anyhow::bail!(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE);
        }
        RuntimeProtectRecoveryDisposition::SettleCancel(handle) => {
            settle_runtime_custody_protect_session(
                registry,
                journal,
                mint_intent.request_id(),
                handle,
                RuntimeProtectSessionSettlementOp::Cancel,
            )
            .await?;
            anyhow::bail!(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE);
        }
        RuntimeProtectRecoveryDisposition::SettleClose(handle) => {
            settle_runtime_custody_protect_session(
                registry,
                journal,
                mint_intent.request_id(),
                handle,
                RuntimeProtectSessionSettlementOp::Close,
            )
            .await?;
            anyhow::bail!(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE);
        }
        RuntimeProtectRecoveryDisposition::ReplayOpenAndSettleCancel => {
            if !registry.has_provider(PROTECT_PROVIDER_ID).await {
                anyhow::bail!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE);
            }
            let opened =
                invoke_typed_protect_provider(registry, "open_protection_session", &open_request)
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE)
                    })?;
            if opened.status() != ProtectProviderResponseStatusV1::ProtectionSessionOpened {
                anyhow::bail!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE);
            }
            let handle = opened
                .protection_session_handle()
                .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE))?
                .ok_or_else(|| {
                    anyhow::anyhow!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE)
                })?;
            journal
                .mark_intent_protect_opened(mint_intent.request_id(), handle)
                .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
            settle_runtime_custody_protect_session(
                registry,
                journal,
                mint_intent.request_id(),
                handle,
                RuntimeProtectSessionSettlementOp::Cancel,
            )
            .await?;
            anyhow::bail!(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE);
        }
        RuntimeProtectRecoveryDisposition::Fresh => {}
    }

    if !registry.has_provider(PROTECT_PROVIDER_ID).await {
        anyhow::bail!("Runtime custody protect provider is unavailable");
    }
    journal
        .mark_intent_protect_effect_started(mint_intent.request_id())
        .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
    let opened =
        invoke_typed_protect_provider(registry, "open_protection_session", &open_request).await?;
    if opened.status() != ProtectProviderResponseStatusV1::ProtectionSessionOpened {
        anyhow::bail!("Runtime custody protect provider is unavailable");
    }
    let handle = opened
        .protection_session_handle()
        .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody protect output is invalid"))?;
    journal
        .mark_intent_protect_opened(mint_intent.request_id(), handle)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
    let protect_result = protect_opened_runtime_custody_session(
        registry,
        journal,
        mint_intent.request_id(),
        handle,
        &opened,
        input,
        composition,
        mint_intent.content_access_id(),
    )
    .await;
    match protect_result {
        Ok(protected) => {
            settle_runtime_custody_protect_session(
                registry,
                journal,
                mint_intent.request_id(),
                handle,
                RuntimeProtectSessionSettlementOp::Close,
            )
            .await?;
            Ok(protected)
        }
        Err(error) => {
            settle_runtime_custody_protect_session(
                registry,
                journal,
                mint_intent.request_id(),
                handle,
                RuntimeProtectSessionSettlementOp::Cancel,
            )
            .await
            .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE))?;
            Err(error)
        }
    }
}

fn runtime_protect_recovery_disposition(
    mint_intent: &RuntimeMintIntent,
) -> RuntimeProtectRecoveryDisposition {
    if let Some(handle) = mint_intent.protect_pending_close_handle() {
        RuntimeProtectRecoveryDisposition::SettleClose(handle)
    } else if let Some(handle) = mint_intent.protect_pending_cancel_handle() {
        RuntimeProtectRecoveryDisposition::SettleCancel(handle)
    } else if mint_intent.protect_open_request_pending() {
        RuntimeProtectRecoveryDisposition::ReplayOpenAndSettleCancel
    } else if mint_intent.protect_terminal_before_draft() {
        RuntimeProtectRecoveryDisposition::TerminalAbort
    } else {
        RuntimeProtectRecoveryDisposition::Fresh
    }
}

fn runtime_custody_open_protection_session_request(
    composition: &RuntimeCustodyComposition,
    input: &RuntimeCustodyLibraryPublishInput,
    mint_intent: &RuntimeMintIntent,
) -> anyhow::Result<ProtectProviderRequestV1> {
    let nodes = composition
        .nodes
        .iter()
        .map(|node| {
            ProtectionSessionNodeV1::new(node.node_public_key, node.custody_public_key)
                .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let segment_count = u32::try_from(input.clear_segments.len())
        .map_err(|_| anyhow::anyhow!("Runtime custody protect input is invalid"))?;
    ProtectProviderRequestV1::new_open_protection_session(
        mint_intent.request_id(),
        mint_intent.content_access_id(),
        composition
            .signed_pool
            .pool_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))?,
        composition
            .signed_epoch
            .epoch_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))?,
        composition
            .signed_committee_authorization
            .authorization_identity()
            .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))?,
        input.mime_type.clone(),
        input.codecs.clone(),
        segment_count,
        &input.clear_init_segment,
        nodes,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))
}

enum RuntimeProtectSessionSettlementOp {
    Cancel,
    Close,
}

async fn settle_runtime_custody_protect_session(
    registry: &ProviderRegistry,
    journal: &RuntimeMintJournal,
    request_id: Digest32,
    handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    op: RuntimeProtectSessionSettlementOp,
) -> anyhow::Result<()> {
    let (provider_op, request) = match op {
        RuntimeProtectSessionSettlementOp::Cancel => (
            "cancel_protection_session",
            ProtectProviderRequestV1::new_cancel_protection_session(handle)
                .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))?,
        ),
        RuntimeProtectSessionSettlementOp::Close => (
            "close_protection_session",
            ProtectProviderRequestV1::new_close_protection_session(handle)
                .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))?,
        ),
    };
    let response = invoke_typed_protect_provider(registry, provider_op, &request).await?;
    match (op, response.status()) {
        (
            RuntimeProtectSessionSettlementOp::Cancel,
            ProtectProviderResponseStatusV1::ProtectionSessionCancelled,
        ) => {
            journal
                .mark_intent_protect_cancelled_before_draft(request_id)
                .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
        }
        (
            RuntimeProtectSessionSettlementOp::Close,
            ProtectProviderResponseStatusV1::ProtectionSessionClosed,
        ) => {
            journal
                .mark_intent_protect_closed_before_draft(request_id)
                .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
        }
        (
            RuntimeProtectSessionSettlementOp::Cancel | RuntimeProtectSessionSettlementOp::Close,
            ProtectProviderResponseStatusV1::ProtectionSessionAlreadyAbsent,
        ) => {
            journal
                .mark_intent_protect_already_absent_before_draft(request_id)
                .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
        }
        _ => anyhow::bail!(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE),
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "This operation keeps protect-session authority and settlement inputs explicit at one boundary"
)]
async fn protect_opened_runtime_custody_session(
    registry: &ProviderRegistry,
    journal: &RuntimeMintJournal,
    request_id: Digest32,
    handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    opened: &ProtectProviderResponseV1,
    input: &RuntimeCustodyLibraryPublishInput,
    composition: &RuntimeCustodyComposition,
    content_access_id: ContentAccessIdV1,
) -> anyhow::Result<ProtectedRuntimeCustodyMedia> {
    let init_segment = opened
        .protected_init_segment()
        .ok_or_else(|| anyhow::anyhow!("Runtime custody protect output is invalid"))?
        .to_vec();
    let mut encrypted_segments = Vec::with_capacity(input.clear_segments.len());
    for (segment_index, clear_segment) in input.clear_segments.iter().enumerate() {
        let protected = invoke_typed_protect_provider(
            registry,
            "protect_media_segment",
            &ProtectProviderRequestV1::new_protect_media_segment(
                handle,
                u32::try_from(segment_index)
                    .map_err(|_| anyhow::anyhow!("Runtime custody protect input is invalid"))?,
                clear_segment,
            )
            .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))?,
        )
        .await?;
        if protected.status() != ProtectProviderResponseStatusV1::MediaSegmentProtected
            || protected.segment_index()
                != Some(
                    u32::try_from(segment_index)
                        .map_err(|_| anyhow::anyhow!("Runtime custody protect input is invalid"))?,
                )
        {
            anyhow::bail!("Runtime custody protect provider is unavailable");
        }
        encrypted_segments.push(
            protected
                .protected_segment()
                .ok_or_else(|| anyhow::anyhow!("Runtime custody protect output is invalid"))?
                .to_vec(),
        );
    }
    let finalized = invoke_typed_protect_provider(
        registry,
        "finalize_protection_session",
        &ProtectProviderRequestV1::new_finalize_protection_session(handle)
            .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))?,
    )
    .await?;
    if finalized.status() != ProtectProviderResponseStatusV1::ProtectionSessionFinalized {
        anyhow::bail!("Runtime custody protect provider is unavailable");
    }
    let media_identity = finalized
        .media_identity()
        .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody protect output is invalid"))?;
    let envelope = finalized
        .custody_envelope()
        .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody protect output is invalid"))?;
    let expected_media = CencFmp4MediaIdentityV1::new_from_bytes(
        &init_segment,
        &encrypted_segments,
        input.mime_type.clone(),
        input.codecs.clone(),
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?;
    let protected_session =
        ValidatedCencFmp4MediaSessionLayoutV1::new(&expected_media, &init_segment)
            .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?;
    if media_identity != expected_media
        || protected_session.content_access_id() != content_access_id
        || envelope.manifest().encrypted_content() != media_identity.encrypted_content()
        || envelope.manifest().custody_pool()
            != composition
                .signed_pool
                .pool_identity()
                .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))?
        || envelope.manifest().custody_epoch()
            != composition
                .signed_epoch
                .epoch_identity()
                .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))?
        || envelope.manifest().custody_committee_authorization()
            != composition
                .signed_committee_authorization
                .authorization_identity()
                .map_err(|_| anyhow::anyhow!("Runtime custody protect committee is invalid"))?
    {
        anyhow::bail!("Runtime custody protect output is invalid");
    }
    journal
        .mark_intent_protect_finalized(request_id, handle)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint intent is unavailable"))?;
    Ok(ProtectedRuntimeCustodyMedia {
        init_segment,
        encrypted_segments,
        media_identity,
        envelope,
    })
}

async fn invoke_typed_protect_provider(
    registry: &ProviderRegistry,
    op: &str,
    request: &ProtectProviderRequestV1,
) -> anyhow::Result<ProtectProviderResponseV1> {
    let request_value = serde_json::from_slice(
        &request
            .to_json_vec()
            .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))?,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody protect request is invalid"))?;
    let data = invoke_json_provider(registry, PROTECT_PROVIDER_ID, op, request_value)
        .await
        .map_err(|_| anyhow::anyhow!("Runtime custody protect provider is unavailable"))?;
    ProtectProviderResponseV1::from_json_slice(
        &serde_json::to_vec(&data)
            .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))
}

fn write_protected_content_staging_directory(
    data_dir: &Path,
    protected: &ProtectedRuntimeCustodyMedia,
) -> anyhow::Result<tempfile::TempDir> {
    let parent = protected_content_root(data_dir);
    fs::create_dir_all(&parent)?;
    let staging = tempfile::Builder::new()
        .prefix("publish-")
        .tempdir_in(&parent)
        .map_err(|_| anyhow::anyhow!("Runtime custody publish staging is unavailable"))?;
    let tree = staging.path().join("protected-content/v1/segments");
    fs::create_dir_all(&tree)
        .map_err(|_| anyhow::anyhow!("Runtime custody publish staging is unavailable"))?;
    fs::write(
        staging.path().join(PROTECTED_CONTENT_IDENTITY_PATH),
        protected
            .media_identity
            .canonical_bytes()
            .map_err(|_| anyhow::anyhow!("Runtime custody protect output is invalid"))?,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody publish staging is unavailable"))?;
    fs::write(
        staging.path().join(PROTECTED_CONTENT_INIT_PATH),
        &protected.init_segment,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody publish staging is unavailable"))?;
    for (index, segment) in protected.encrypted_segments.iter().enumerate() {
        fs::write(
            staging.path().join(protected_content_segment_path(index)),
            segment,
        )
        .map_err(|_| anyhow::anyhow!("Runtime custody publish staging is unavailable"))?;
    }
    Ok(staging)
}

fn runtime_custody_library_publish_facts(
    input: &RuntimeCustodyLibraryPublishInput,
    draft: &RuntimeMintDraft,
    content_id: &str,
    evidence: &RuntimeVerifiedContentAvailability,
) -> RuntimeCustodyLibraryPublishFacts {
    let mint_id_hex = hex::encode(draft.mint_id().as_bytes());
    let receipt_digest_hex = hex::encode(evidence.receipt_digest().as_bytes());
    let availability = json!({
        "schema": "elastos.library.runtime-custody-availability/v1",
        "cid": evidence.content_cid(),
        "content_id": content_id,
        "mint_id": mint_id_hex,
        "status": PROTECTED_CONTENT_AVAILABLE_STATUS,
        "replicas": evidence.observed_replicas(),
        "checked_at": evidence.checked_at(),
        "receipt_digest": receipt_digest_hex,
    });
    RuntimeCustodyLibraryPublishFacts {
        content_cid: evidence.content_cid().to_string(),
        mint_id: draft.mint_id(),
        content_id: content_id.to_string(),
        availability: availability.clone(),
        receipt: json!({
            "schema": "elastos.library.runtime-custody-receipt/v1",
            "receipt_digest": receipt_digest_hex,
        }),
        content_security: json!({
            "schema": "elastos.library.published-content-security/v1",
            "object_uri": input.object_uri,
            "source_storage": input.source_storage,
            "published_payload": "runtime_custody_encrypted",
            "key_release_required": false,
            "status": "runtime_custody_available",
            "content_id": content_id,
            "mint_id": mint_id_hex,
            "required_providers": [],
        }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeCustodyListingRecord {
    pub(crate) schema: String,
    pub(crate) mint_id: String,
    pub(crate) content_id: String,
    pub(crate) content_access_id: String,
    pub(crate) cid: String,
    pub(crate) metadata_cid: String,
    pub(crate) token_uri: String,
    pub(crate) publisher_principal_id: String,
    pub(crate) seller_address: String,
    pub(crate) chain_namespace: String,
    pub(crate) network: String,
    pub(crate) ledger: String,
    pub(crate) token_id: String,
    pub(crate) operative: String,
    pub(crate) price: String,
    pub(crate) pay_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) payment_processor: Option<String>,
    pub(crate) published_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeCustodyPurchaseStageRecord {
    pub(crate) stage: String,
    pub(crate) effect_id: String,
    pub(crate) approval_request_id: String,
    pub(crate) request_sha256: String,
    pub(crate) chain_namespace: String,
    pub(crate) network: String,
    pub(crate) to: String,
    pub(crate) value: String,
    pub(crate) data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeCustodyConfirmedPurchaseStage {
    pub(crate) chain_transaction: String,
    pub(crate) wallet_binding: ValidatedChainOutcomeBindingV1,
    pub(crate) chain_observation: Value,
    pub(crate) confirmed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeCustodyPurchaseAccessEvidenceRecord {
    pub(crate) schema: String,
    pub(crate) request_id: String,
    pub(crate) network: String,
    pub(crate) chain_id: u64,
    pub(crate) wallet: String,
    pub(crate) content_access_id: String,
    pub(crate) has_access: bool,
    pub(crate) finalized_block_number: u64,
    pub(crate) finalized_block_hash: String,
    pub(crate) finalized_block_timestamp: u64,
    pub(crate) observed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeCustodyTerminalPurchaseRecord {
    pub(crate) chain_transaction: String,
    pub(crate) wallet_binding: ValidatedChainOutcomeBindingV1,
    pub(crate) chain_observation: Value,
    pub(crate) access_evidence: RuntimeCustodyPurchaseAccessEvidenceRecord,
    pub(crate) confirmed_at: u64,
    pub(crate) bought_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RuntimeCustodyPurchaseProgress {
    Pending {
        #[serde(skip_serializing_if = "Option::is_none")]
        confirmed_buy: Option<RuntimeCustodyConfirmedPurchaseStage>,
    },
    Complete {
        terminal: RuntimeCustodyTerminalPurchaseRecord,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeCustodyPurchaseRecord {
    pub(crate) schema: String,
    pub(crate) principal_id: String,
    pub(crate) profile_did: String,
    pub(crate) mint_id: String,
    pub(crate) content_id: String,
    pub(crate) cid: String,
    pub(crate) listing_sha256: String,
    pub(crate) seller_address: String,
    pub(crate) chain_namespace: String,
    pub(crate) network: String,
    pub(crate) ledger: String,
    pub(crate) token_id: String,
    pub(crate) operative: String,
    pub(crate) price: String,
    pub(crate) pay_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) payment_processor: Option<String>,
    pub(crate) availability_receipt_digest: String,
    pub(crate) account_id: String,
    pub(crate) address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approval_stage: Option<RuntimeCustodyPurchaseStageRecord>,
    pub(crate) buy_stage: RuntimeCustodyPurchaseStageRecord,
    pub(crate) progress: RuntimeCustodyPurchaseProgress,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

pub(crate) struct RuntimeCustodyBuyInput {
    pub principal_id: String,
    pub mint_id: String,
    pub account_id: String,
}

pub(crate) struct RuntimeCustodyViewerOpenInput {
    pub principal_id: String,
    pub mint_id: String,
    pub proof_binding_id: Option<String>,
    pub session_id: Option<String>,
    pub grant_id: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeCustodyViewerRecord {
    schema: String,
    principal_id: String,
    profile_did: String,
    mint_id: String,
    content_id: String,
    runtime_session_binding_digest: String,
    audit_request_id: String,
    viewer_session_handle: String,
    expires_at: u64,
    lifecycle_status: RuntimeCustodyViewerLifecycleStatus,
    pending_close_result: Option<RuntimeCustodyOpenPendingCloseResult>,
    pending_cancel_result: Option<RuntimeCustodyOpenPendingCancelResult>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeCustodyViewerLifecycleStatus {
    OpenPending,
    Active,
    CleanupPending,
    Closed,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeCustodyOpenPendingCloseResult {
    Closed,
    AlreadyAbsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeCustodyOpenPendingCancelResult {
    Cancelled,
    AlreadyAbsent,
}

struct RuntimeCustodyOpenPendingInput<'a> {
    principal_id: &'a str,
    profile_did: &'a str,
    mint_id: Digest32,
    content_id: &'a str,
    runtime_session_binding: RuntimeSessionBindingV1,
    audit_request_id: Digest32,
    viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    expires_at: u64,
    now: u64,
}

impl RuntimeCustodyViewerRecord {
    fn from_open_pending(input: RuntimeCustodyOpenPendingInput<'_>) -> anyhow::Result<Self> {
        let record = Self {
            schema: RUNTIME_VIEWER_SCHEMA_V1.to_string(),
            principal_id: input.principal_id.to_string(),
            profile_did: input.profile_did.to_string(),
            mint_id: hex::encode(input.mint_id.as_bytes()),
            content_id: input.content_id.to_string(),
            runtime_session_binding_digest: hex::encode(
                input.runtime_session_binding.digest().as_bytes(),
            ),
            audit_request_id: hex::encode(input.audit_request_id.as_bytes()),
            viewer_session_handle: hex::encode(input.viewer_session_handle),
            expires_at: input.expires_at,
            lifecycle_status: RuntimeCustodyViewerLifecycleStatus::OpenPending,
            pending_close_result: None,
            pending_cancel_result: None,
            created_at: input.now,
            updated_at: input.now,
        };
        let _ = record.audit_request_id()?;
        let _ = record.viewer_session_handle_bytes()?;
        if record.expires_at == 0 {
            anyhow::bail!("Runtime custody viewer session is unavailable");
        }
        Ok(record)
    }

    fn from_active_session(
        principal_id: &str,
        profile_did: &str,
        mint_id: Digest32,
        content_id: &str,
        runtime_session_binding: RuntimeSessionBindingV1,
        session: &RuntimeViewerSession,
        now: u64,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            schema: RUNTIME_VIEWER_SCHEMA_V1.to_string(),
            principal_id: principal_id.to_string(),
            profile_did: profile_did.to_string(),
            mint_id: hex::encode(mint_id.as_bytes()),
            content_id: content_id.to_string(),
            runtime_session_binding_digest: hex::encode(
                runtime_session_binding.digest().as_bytes(),
            ),
            audit_request_id: hex::encode(session.audit_request_id().as_bytes()),
            viewer_session_handle: hex::encode(session.viewer_session_handle()),
            expires_at: session.expires_at(),
            lifecycle_status: RuntimeCustodyViewerLifecycleStatus::Active,
            pending_close_result: None,
            pending_cancel_result: None,
            created_at: now,
            updated_at: now,
        })
    }

    fn validates_authority_identity(
        &self,
        principal_id: &str,
        profile_did: &str,
        mint_id: Digest32,
        content_id: &str,
    ) -> bool {
        if self.schema != RUNTIME_VIEWER_SCHEMA_V1
            || self.principal_id != principal_id
            || self.profile_did != profile_did
            || self.mint_id != hex::encode(mint_id.as_bytes())
            || self.content_id != content_id
        {
            return false;
        }
        true
    }

    fn matches_runtime_session_binding(
        &self,
        runtime_session_binding: &RuntimeSessionBindingV1,
    ) -> bool {
        self.runtime_session_binding_digest
            == hex::encode(runtime_session_binding.digest().as_bytes())
    }

    fn audit_request_id(&self) -> anyhow::Result<Digest32> {
        Ok(Digest32::new(
            decode_hex_bytes(&self.audit_request_id)
                .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?
                .try_into()
                .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?,
        ))
    }

    fn viewer_session_handle_bytes(
        &self,
    ) -> anyhow::Result<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]> {
        decode_hex_bytes(&self.viewer_session_handle)
            .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?
            .try_into()
            .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))
    }

    fn to_runtime_viewer_session(
        &self,
        mint: &PersistedRuntimeMint,
    ) -> anyhow::Result<RuntimeViewerSession> {
        RuntimeViewerSession::from_persisted_parts(
            self.audit_request_id()?,
            self.viewer_session_handle_bytes()?,
            mint.draft().encrypted_content().clone(),
            RightsActionV1::View,
            self.expires_at,
        )
        .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))
    }

    fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    fn open_pending_close_result(&self) -> Option<RuntimeCustodyOpenPendingCloseResult> {
        self.pending_close_result
    }

    fn open_pending_cancel_result(&self) -> Option<RuntimeCustodyOpenPendingCancelResult> {
        self.pending_cancel_result
    }

    fn mark_open_pending_close_result(
        &mut self,
        result: RuntimeViewerSessionCloseResult,
        now: u64,
    ) {
        self.pending_close_result = Some(match result {
            RuntimeViewerSessionCloseResult::Closed => RuntimeCustodyOpenPendingCloseResult::Closed,
            RuntimeViewerSessionCloseResult::AlreadyAbsent => {
                RuntimeCustodyOpenPendingCloseResult::AlreadyAbsent
            }
        });
        self.updated_at = now;
    }

    fn mark_open_pending_cancel_result(
        &mut self,
        result: RuntimePreparedRecipientCancelResult,
        now: u64,
    ) {
        self.pending_cancel_result = Some(match result {
            RuntimePreparedRecipientCancelResult::Cancelled => {
                RuntimeCustodyOpenPendingCancelResult::Cancelled
            }
            RuntimePreparedRecipientCancelResult::AlreadyAbsent => {
                RuntimeCustodyOpenPendingCancelResult::AlreadyAbsent
            }
        });
        self.updated_at = now;
    }

    fn mark_cleanup_pending(&mut self, now: u64) {
        self.lifecycle_status = RuntimeCustodyViewerLifecycleStatus::CleanupPending;
        self.pending_close_result = None;
        self.pending_cancel_result = None;
        self.updated_at = now;
    }

    fn mark_terminal(&mut self, result: RuntimeViewerSessionCloseResult, now: u64) {
        self.lifecycle_status = match result {
            RuntimeViewerSessionCloseResult::Closed => RuntimeCustodyViewerLifecycleStatus::Closed,
            RuntimeViewerSessionCloseResult::AlreadyAbsent => {
                RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
            }
        };
        self.pending_close_result = None;
        self.pending_cancel_result = None;
        self.updated_at = now;
    }
}

pub(crate) fn list_runtime_custody_listings(
    data_dir: &Path,
    principal_id: &str,
) -> anyhow::Result<Value> {
    if principal_id.trim().is_empty() {
        anyhow::bail!("Runtime custody listing principal is invalid");
    }
    let root = data_dir.join(RUNTIME_LISTING_ROOT);
    let mut listings = Vec::new();
    if root.is_dir() {
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let record: RuntimeCustodyListingRecord =
                serde_json::from_slice(&fs::read(entry.path())?)?;
            if record.schema != RUNTIME_LISTING_SCHEMA_V1 {
                anyhow::bail!("Runtime custody listing is invalid");
            }
            listings.push(json!({
                "schema": RUNTIME_LISTING_SCHEMA_V1,
                "mint_id": record.mint_id,
                "content_id": record.content_id,
                "cid": record.cid,
                "publisher_principal_id": record.publisher_principal_id,
                "published_at": record.published_at,
                "availability": {
                    "schema": "elastos.library.runtime-custody-availability/v1",
                    "status": PROTECTED_CONTENT_AVAILABLE_STATUS,
                    "cid": record.cid,
                    "content_id": record.content_id,
                    "mint_id": record.mint_id,
                },
            }));
        }
    }
    listings.sort_by(|left, right| {
        left.get("mint_id")
            .and_then(Value::as_str)
            .cmp(&right.get("mint_id").and_then(Value::as_str))
    });
    Ok(json!({
        "schema": "elastos.library.runtime-custody-listings/v1",
        "listings": listings,
    }))
}

pub(crate) async fn verify_fresh_runtime_custody_buy_availability(
    data_dir: &Path,
    registry: &ProviderRegistry,
    mint_id: Digest32,
    now_unix_seconds: u64,
) -> anyhow::Result<RuntimeVerifiedContentAvailability> {
    let expected_mint_id = hex::encode(mint_id.as_bytes());
    let listing = load_runtime_custody_listing(data_dir, mint_id)?
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE))?;
    if listing.mint_id != expected_mint_id {
        anyhow::bail!(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE);
    }
    let mint = runtime_mint_journal(data_dir)
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE))?;
    let persisted = mint
        .content_availability()
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE))?;
    let expected_content_id = runtime_protected_content_id(mint.draft().encrypted_content())
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE))?;
    if listing.content_id != expected_content_id || listing.cid != persisted.content_cid() {
        anyhow::bail!(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE);
    }
    let requirement = RuntimeContentAvailabilityRequirement::new(
        persisted.expected_provider_did(),
        persisted.object_identity(),
        persisted.publisher_identity(),
        persisted.policy(),
        persisted.required_replicas(),
        PROTECTED_CONTENT_AVAILABILITY_MAX_AGE_SECS,
        PROTECTED_CONTENT_AVAILABILITY_MAX_FUTURE_SKEW_SECS,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    let content_cid = persisted.content_cid();
    let receipt = fetch_content_availability_receipt(registry, content_cid)
        .await
        .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    let manifest = crate::content::fetch_content_object_manifest(registry, content_cid)
        .await
        .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    verify_protected_content_manifest_and_files(
        registry,
        content_cid,
        &manifest,
        mint.draft().media_identity(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    let verified = verify_protected_content_receipt(
        content_cid,
        &manifest,
        &receipt,
        mint.draft().media_identity(),
        &requirement,
        now_unix_seconds,
    )
    .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    if verified.content_cid() != listing.cid
        || verified.encrypted_content() != mint.draft().encrypted_content()
        || verified.media_manifest_root() != mint.draft().media_identity().media_manifest_root()
    {
        anyhow::bail!("Runtime custody content availability is unavailable");
    }
    Ok(verified)
}

pub(crate) async fn open_runtime_custody_viewer(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    input: RuntimeCustodyViewerOpenInput,
) -> anyhow::Result<Value> {
    let mint_id = parse_mint_id_hex(&input.mint_id)?;
    let _viewer_lifecycle_guard =
        acquire_runtime_custody_viewer_lifecycle_guard(data_dir, &input.principal_id, mint_id)
            .await;
    let purchase = load_runtime_custody_purchase(data_dir, &input.principal_id, mint_id)?
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE))?;
    if purchase.principal_id != input.principal_id {
        anyhow::bail!(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE);
    }
    let proof_binding_id = input
        .proof_binding_id
        .as_deref()
        .filter(|value| !value.is_empty());
    let session_id = input
        .session_id
        .as_deref()
        .filter(|value| !value.is_empty());
    let grant_id = input.grant_id.as_deref().filter(|value| !value.is_empty());
    let (Some(proof_binding_id), Some(session_id), Some(grant_id)) =
        (proof_binding_id, session_id, grant_id)
    else {
        anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
    };
    let profile_did = load_runtime_custody_profile_did(data_dir, &input.principal_id)?;
    let mint = runtime_mint_journal(data_dir)
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint selection is invalid"))?;
    let buy = reconstructed_buy_receipt(&mint, &purchase, &profile_did)?;
    let runtime_session_binding = derive_runtime_custody_session_binding(
        &input.principal_id,
        &profile_did,
        proof_binding_id,
        session_id,
        grant_id,
        mint_id,
    )?;
    if let Some(record) =
        load_runtime_custody_viewer_record(data_dir, &input.principal_id, mint_id)?
    {
        if !record.validates_authority_identity(
            &input.principal_id,
            &profile_did,
            mint_id,
            &purchase.content_id,
        ) {
            anyhow::bail!(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE);
        }
        let viewer_now = crate::auth::now_ts();
        match record.lifecycle_status {
            RuntimeCustodyViewerLifecycleStatus::Active
                if record.matches_runtime_session_binding(&runtime_session_binding)
                    && !record.is_expired(viewer_now) =>
            {
                let session = record.to_runtime_viewer_session(&mint)?;
                return Ok(json!({
                    "schema": "elastos.library.runtime-custody-viewer/v1",
                    "mint_id": hex::encode(mint_id.as_bytes()),
                    "viewer_session_handle": hex::encode(session.viewer_session_handle()),
                    "expires_at": session.expires_at(),
                }));
            }
            RuntimeCustodyViewerLifecycleStatus::Active
                if !record.matches_runtime_session_binding(&runtime_session_binding)
                    && !record.is_expired(viewer_now) =>
            {
                anyhow::bail!(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE);
            }
            RuntimeCustodyViewerLifecycleStatus::OpenPending
            | RuntimeCustodyViewerLifecycleStatus::Active
            | RuntimeCustodyViewerLifecycleStatus::CleanupPending => {
                if record.lifecycle_status == RuntimeCustodyViewerLifecycleStatus::Active
                    && !record.is_expired(viewer_now)
                    && !record.matches_runtime_session_binding(&runtime_session_binding)
                {
                    anyhow::bail!(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE);
                }
                let _ = settle_runtime_custody_viewer_cleanup(
                    data_dir,
                    registry.clone(),
                    &input.principal_id,
                    mint_id,
                    record,
                )
                .await?;
            }
            RuntimeCustodyViewerLifecycleStatus::Closed
            | RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent => {}
        }
    }
    let composition = load_runtime_custody_composition(data_dir, registry.clone())?
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE))?;
    let (device_key, _) =
        crate::collaboration_profile_authority::load_existing_device_signing_key(data_dir)?
            .ok_or_else(|| anyhow::anyhow!("local Runtime device signing key is missing"))?;
    let runtime_issuer = RuntimeOperationIssuerKeyV1::new(device_key.verifying_key().to_bytes())
        .map_err(|_| anyhow::anyhow!("local Runtime device signing key is invalid"))?;
    let envelope = load_runtime_open_envelope(data_dir, mint_id)?
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_VIEWER_ENVELOPE_UNAVAILABLE_MESSAGE))?;
    let (media_identity, protected_init) = fetch_runtime_custody_open_media(
        registry.as_ref(),
        &purchase.cid,
        mint.draft().media_identity(),
    )
    .await?;
    let now = crate::auth::now_ts();
    let audit_request_id = runtime_open_audit_id(&input.principal_id, mint_id, now);
    let decrypt = RuntimeDecryptRegistryAdapter::new(registry.clone());
    let prepared = match prepare_recipient(
        &decrypt,
        &buy,
        runtime_session_binding,
        audit_request_id,
        runtime_issuer,
        now.saturating_sub(5),
        now + 60,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(_) => anyhow::bail!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE),
    };
    let open_pending =
        RuntimeCustodyViewerRecord::from_open_pending(RuntimeCustodyOpenPendingInput {
            principal_id: &input.principal_id,
            profile_did: &profile_did,
            mint_id,
            content_id: &purchase.content_id,
            runtime_session_binding,
            audit_request_id: prepared.audit_request_id(),
            viewer_session_handle: *prepared.prepared_recipient_handle(),
            expires_at: prepared.expires_at(),
            now: crate::auth::now_ts(),
        })?;
    if let Err(error) =
        persist_runtime_custody_viewer_record(data_dir, &input.principal_id, mint_id, &open_pending)
    {
        let _ = cancel_prepared_recipient(&decrypt, &prepared).await;
        return Err(error);
    }
    let (release_wallet_request, release_wallet_response, signed_rights) =
        match invoke_runtime_release_wallet(
            registry.as_ref(),
            &buy,
            prepared.recipient_identity(),
            RuntimeReleaseWalletInvocation {
                principal_id: &input.principal_id,
                account_id: &purchase.account_id,
                proof_binding_id,
                session_id,
                grant_id,
                mint_id,
                runtime_session_binding,
            },
            now,
        )
        .await
        {
            Ok(bundle) => bundle,
            Err(_) => {
                let _ = settle_runtime_custody_viewer_cleanup(
                    data_dir,
                    registry.clone(),
                    &input.principal_id,
                    mint_id,
                    open_pending.clone(),
                )
                .await;
                anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
            }
        };
    let policy = match resolve_runtime_rights_policy(
        registry.as_ref(),
        mint.draft().encrypted_content(),
        mint.draft().content_access_id(),
        RightsActionV1::View,
    )
    .await
    {
        Ok(policy) => policy,
        Err(_) => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!("Runtime custody rights policy is unavailable");
        }
    };
    let release_request = match KeyReleaseRequestV1::new(
        signed_rights.request().binding().clone(),
        signed_rights
            .request()
            .request_hash()
            .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?,
        RightsActionV1::View,
        prepared.recipient_identity().clone(),
        now.saturating_sub(5),
        now + 55,
        ReplayNonce16::new(audit_nonce_bytes(audit_request_id)),
    ) {
        Ok(request) => request,
        Err(_) => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
        }
    };
    let evidence_request = match RightsEvaluationEvidenceRequestV1::new(
        signed_rights.request().binding().clone(),
        policy.identity().clone(),
    ) {
        Ok(request) => request,
        Err(_) => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
        }
    };
    let localhost_root = crate::auth::principal_localhost_root(&input.principal_id);
    let operation = match assemble_protected_content_runtime_release_operation(
        data_dir,
        &input.principal_id,
        &localhost_root,
        proof_binding_id,
        RuntimeReleaseOperationAssemblyInput {
            rights_request: signed_rights,
            release_request,
            recipient_public_key: *prepared.recipient_public_key(),
            recipient_identity: prepared.recipient_identity().clone(),
            policy_body: policy.body().clone(),
            evidence_request,
            custody_epoch: composition.signed_epoch.clone(),
            audit_request_id,
            issued_at: now.saturating_sub(3),
            expires_at: now + 50,
        },
        now,
    ) {
        Ok(operation) => operation,
        Err(_) => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
        }
    };
    let selected = vec![
        RuntimeSelectedProvider::new(
            composition.nodes[0].node_public_key,
            &composition.nodes[0].adapter,
            &composition.nodes[0].adapter,
        ),
        RuntimeSelectedProvider::new(
            composition.nodes[1].node_public_key,
            &composition.nodes[1].adapter,
            &composition.nodes[1].adapter,
        ),
        RuntimeSelectedProvider::new(
            composition.nodes[2].node_public_key,
            &composition.nodes[2].adapter,
            &composition.nodes[2].adapter,
        ),
    ];
    let coordinator =
        RuntimeReleaseCoordinator::new(runtime_release_journal(data_dir), runtime_issuer, selected)
            .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?
            .with_response_clock(crate::auth::now_ts);
    let contributions = match coordinator
        .release(
            &release_wallet_request,
            &release_wallet_response,
            operation.clone(),
            now,
        )
        .await
    {
        Ok(RuntimeReleaseCoordinatorOutcome::Terminal(
            RuntimeReleaseTerminalResult::ContributionsReady {
                signed_node_contributions,
            },
        )) => signed_node_contributions,
        Ok(RuntimeReleaseCoordinatorOutcome::Nonterminal { operation_hash, .. }) => {
            match coordinator
                .resume_exact(operation_hash, crate::auth::now_ts())
                .await
            {
                Ok(RuntimeReleaseCoordinatorOutcome::Terminal(
                    RuntimeReleaseTerminalResult::ContributionsReady {
                        signed_node_contributions,
                    },
                )) => signed_node_contributions,
                _ => {
                    let _ = settle_runtime_custody_viewer_cleanup(
                        data_dir,
                        registry.clone(),
                        &input.principal_id,
                        mint_id,
                        open_pending.clone(),
                    )
                    .await;
                    anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
                }
            }
        }
        _ => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
        }
    };
    let receipt_now = crate::auth::now_ts();
    let terminal_receipt = match sign_runtime_terminal_receipt(
        &device_key,
        &operation,
        &contributions,
        &composition.signed_epoch,
        receipt_now,
    ) {
        Ok(receipt) => receipt,
        Err(_) => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
        }
    };
    let expected_terminal_issuer =
        match TerminalReceiptIssuerKey::new(device_key.verifying_key().to_bytes()) {
            Ok(issuer) => issuer,
            Err(_) => {
                let _ = settle_runtime_custody_viewer_cleanup(
                    data_dir,
                    registry.clone(),
                    &input.principal_id,
                    mint_id,
                    open_pending.clone(),
                )
                .await;
                anyhow::bail!("local Runtime device signing key is invalid");
            }
        };
    let session = match open_viewer_session(
        &decrypt,
        &RuntimeOpenViewerSessionInput {
            buy: &buy,
            prepared_recipient: &prepared,
            signed_runtime_release_operation: &operation,
            expected_terminal_issuer,
            custody_envelope: &envelope,
            media_identity: &media_identity,
            protected_init_segment: &protected_init,
            signed_node_contributions: &contributions,
            signed_terminal_receipt: &terminal_receipt,
            now_unix_seconds: crate::auth::now_ts(),
        },
    )
    .await
    {
        Ok(session) => session,
        Err(_) => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &input.principal_id,
                mint_id,
                open_pending.clone(),
            )
            .await;
            anyhow::bail!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE);
        }
    };
    let handle = *session.viewer_session_handle();
    let expires_at = session.expires_at();
    let viewer_now = crate::auth::now_ts();
    let record = RuntimeCustodyViewerRecord::from_active_session(
        &input.principal_id,
        &profile_did,
        mint_id,
        &purchase.content_id,
        runtime_session_binding,
        &session,
        viewer_now,
    )?;
    if let Err(error) =
        persist_runtime_custody_viewer_record(data_dir, &input.principal_id, mint_id, &record)
    {
        let _ = settle_runtime_custody_viewer_cleanup(
            data_dir,
            registry.clone(),
            &input.principal_id,
            mint_id,
            open_pending,
        )
        .await;
        return Err(error);
    }
    Ok(json!({
        "schema": "elastos.library.runtime-custody-viewer/v1",
        "mint_id": hex::encode(mint_id.as_bytes()),
        "viewer_session_handle": hex::encode(handle),
        "expires_at": expires_at,
    }))
}

pub(crate) async fn read_runtime_custody_viewer(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    mint_id_hex: &str,
    handle_hex: &str,
    segment_index: Option<u32>,
) -> anyhow::Result<Value> {
    let mint_id = parse_mint_id_hex(mint_id_hex)?;
    let _viewer_lifecycle_guard =
        acquire_runtime_custody_viewer_lifecycle_guard(data_dir, principal_id, mint_id).await;
    let handle = parse_viewer_handle_hex(handle_hex)?;
    let purchase = load_runtime_custody_purchase(data_dir, principal_id, mint_id)?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?;
    if purchase.principal_id != principal_id {
        anyhow::bail!("Runtime custody viewer session is unavailable");
    }
    let profile_did = load_runtime_custody_profile_did(data_dir, principal_id)?;
    let mint = runtime_mint_journal(data_dir)
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint selection is invalid"))?;
    let record = load_runtime_custody_viewer_record(data_dir, principal_id, mint_id)?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?;
    if !record.validates_authority_identity(
        principal_id,
        &profile_did,
        mint_id,
        &purchase.content_id,
    ) || record.viewer_session_handle != hex::encode(handle)
    {
        anyhow::bail!("Runtime custody viewer session is unavailable");
    }
    let now = crate::auth::now_ts();
    let session = match record.lifecycle_status {
        RuntimeCustodyViewerLifecycleStatus::Active if !record.is_expired(now) => {
            record.to_runtime_viewer_session(&mint)?
        }
        RuntimeCustodyViewerLifecycleStatus::OpenPending
        | RuntimeCustodyViewerLifecycleStatus::Active
        | RuntimeCustodyViewerLifecycleStatus::CleanupPending => {
            let _ = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                principal_id,
                mint_id,
                record,
            )
            .await?;
            anyhow::bail!("Runtime custody viewer session is unavailable");
        }
        RuntimeCustodyViewerLifecycleStatus::Closed
        | RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent => {
            anyhow::bail!("Runtime custody viewer session is unavailable");
        }
    };
    let selector = if let Some(segment_index) = segment_index {
        let path = protected_content_segment_path(
            usize::try_from(segment_index)
                .map_err(|_| anyhow::anyhow!("Runtime custody viewer media part is invalid"))?,
        );
        if usize::try_from(segment_index).ok()
            >= Some(mint.draft().media_identity().encrypted_segments().len())
        {
            anyhow::bail!("Runtime custody viewer media part is invalid");
        }
        let encrypted =
            crate::content::fetch_bytes_via_provider(registry.as_ref(), &purchase.cid, Some(&path))
                .await
                .map_err(|_| {
                    anyhow::anyhow!("Runtime custody content availability is unavailable")
                })?;
        ViewerMediaPartSelectorV1::segment(segment_index, encrypted)
            .map_err(|_| anyhow::anyhow!("Runtime custody viewer media part is invalid"))?
    } else {
        ViewerMediaPartSelectorV1::init()
    };
    let part = read_viewer_media_part(
        &RuntimeDecryptRegistryAdapter::new(registry),
        &session,
        selector,
        crate::auth::now_ts(),
    )
    .await
    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE))?;
    Ok(json!({
        "schema": "elastos.library.runtime-custody-viewer-part/v1",
        "mint_id": hex::encode(mint_id.as_bytes()),
        "viewer_session_handle": handle_hex,
        "encoding": "base64",
        "data": base64::engine::general_purpose::STANDARD.encode(part.clear_media_part()),
    }))
}

pub(crate) async fn close_runtime_custody_viewer(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    mint_id_hex: &str,
    handle_hex: &str,
) -> anyhow::Result<Value> {
    let mint_id = parse_mint_id_hex(mint_id_hex)?;
    let _viewer_lifecycle_guard =
        acquire_runtime_custody_viewer_lifecycle_guard(data_dir, principal_id, mint_id).await;
    let handle = parse_viewer_handle_hex(handle_hex)?;
    let purchase = load_runtime_custody_purchase(data_dir, principal_id, mint_id)?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?;
    if purchase.principal_id != principal_id {
        anyhow::bail!("Runtime custody viewer session is unavailable");
    }
    let profile_did = load_runtime_custody_profile_did(data_dir, principal_id)?;
    let mint = runtime_mint_journal(data_dir)
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint selection is invalid"))?;
    let mut record = load_runtime_custody_viewer_record(data_dir, principal_id, mint_id)?
        .ok_or_else(|| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?;
    if !record.validates_authority_identity(
        principal_id,
        &profile_did,
        mint_id,
        &purchase.content_id,
    ) || record.viewer_session_handle != hex::encode(handle)
    {
        anyhow::bail!("Runtime custody viewer session is unavailable");
    }
    let result = match record.lifecycle_status {
        RuntimeCustodyViewerLifecycleStatus::Closed
        | RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent => None,
        RuntimeCustodyViewerLifecycleStatus::OpenPending
        | RuntimeCustodyViewerLifecycleStatus::CleanupPending => {
            let settled = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                principal_id,
                mint_id,
                record.clone(),
            )
            .await?;
            Some(match settled.lifecycle_status {
                RuntimeCustodyViewerLifecycleStatus::Closed => {
                    RuntimeViewerSessionCloseResult::Closed
                }
                RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent => {
                    RuntimeViewerSessionCloseResult::AlreadyAbsent
                }
                _ => anyhow::bail!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE),
            })
        }
        RuntimeCustodyViewerLifecycleStatus::Active => {
            let session = record.to_runtime_viewer_session(&mint)?;
            Some(
                match close_viewer_session_with_result(
                    &RuntimeDecryptRegistryAdapter::new(registry),
                    &session,
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        record.mark_cleanup_pending(crate::auth::now_ts());
                        persist_runtime_custody_viewer_record(
                            data_dir,
                            principal_id,
                            mint_id,
                            &record,
                        )?;
                        anyhow::bail!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE);
                    }
                },
            )
        }
    };
    let close_result = if let Some(result) = result {
        record.mark_terminal(result, crate::auth::now_ts());
        persist_runtime_custody_viewer_record(data_dir, principal_id, mint_id, &record)?;
        result
    } else {
        RuntimeViewerSessionCloseResult::AlreadyAbsent
    };
    Ok(json!({
        "schema": "elastos.library.runtime-custody-viewer/v1",
        "mint_id": hex::encode(mint_id.as_bytes()),
        "closed": true,
        "close_result": match close_result {
            RuntimeViewerSessionCloseResult::Closed => "closed",
            RuntimeViewerSessionCloseResult::AlreadyAbsent => "already_absent",
        },
    }))
}

fn persist_runtime_open_envelope(
    data_dir: &Path,
    mint_id: Digest32,
    envelope: &CustodyEnvelopeV1,
) -> anyhow::Result<()> {
    // Owner-only Runtime open material, not a mint journal or Library record.
    // Reconstruct still needs the exact envelope identity until custody-node
    // reassembly exists. Never return these bytes to capsules.
    let bytes = envelope
        .canonical_bytes()
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_VIEWER_ENVELOPE_UNAVAILABLE_MESSAGE))?;
    write_owner_only_bytes(&runtime_open_envelope_path(data_dir, mint_id), &bytes)
}

fn load_runtime_open_envelope(
    data_dir: &Path,
    mint_id: Digest32,
) -> anyhow::Result<Option<CustodyEnvelopeV1>> {
    let path = runtime_open_envelope_path(data_dir, mint_id);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_VIEWER_ENVELOPE_UNAVAILABLE_MESSAGE))?;
    Ok(Some(
        CustodyEnvelopeV1::from_canonical_bytes(&bytes)
            .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_VIEWER_ENVELOPE_UNAVAILABLE_MESSAGE))?,
    ))
}

pub(crate) fn persist_runtime_custody_listing(
    data_dir: &Path,
    record: &RuntimeCustodyListingRecord,
) -> anyhow::Result<()> {
    write_owner_only_bytes(
        &runtime_listing_path(data_dir, parse_mint_id_hex(&record.mint_id)?),
        &serde_json::to_vec(record)?,
    )
}

pub(crate) fn load_runtime_custody_listing(
    data_dir: &Path,
    mint_id: Digest32,
) -> anyhow::Result<Option<RuntimeCustodyListingRecord>> {
    let path = runtime_listing_path(data_dir, mint_id);
    if !path.exists() {
        return Ok(None);
    }
    let record: RuntimeCustodyListingRecord = serde_json::from_slice(&fs::read(path)?)?;
    if record.schema != RUNTIME_LISTING_SCHEMA_V1 {
        anyhow::bail!("Runtime custody listing is invalid");
    }
    Ok(Some(record))
}

pub(crate) fn load_runtime_custody_listing_bytes(
    data_dir: &Path,
    mint_id: Digest32,
) -> anyhow::Result<Option<Vec<u8>>> {
    let path = runtime_listing_path(data_dir, mint_id);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(fs::read(path)?))
}

pub(crate) fn persist_runtime_custody_creator_listing(
    data_dir: &Path,
    mint: &PersistedRuntimeMint,
    facts: &RuntimeCustodyLibraryPublishFacts,
    publisher_principal_id: &str,
    terminal: &RuntimeMintCreatorTerminalEvidence,
) -> anyhow::Result<()> {
    let mint_id = mint.draft().mint_id();
    let expected = RuntimeCustodyListingRecord {
        schema: RUNTIME_LISTING_SCHEMA_V1.to_string(),
        mint_id: hex::encode(mint_id.as_bytes()),
        content_id: facts.content_id.clone(),
        content_access_id: format!(
            "0x{}",
            hex::encode(mint.draft().content_access_id().as_bytes())
        ),
        cid: facts.content_cid.clone(),
        metadata_cid: terminal.metadata_cid().to_string(),
        token_uri: terminal.token_uri().to_string(),
        publisher_principal_id: publisher_principal_id.to_string(),
        seller_address: terminal.seller().to_string(),
        chain_namespace: terminal.chain_namespace().to_string(),
        network: terminal.network().to_string(),
        ledger: terminal.ledger().to_string(),
        token_id: terminal.token_id().to_string(),
        operative: terminal.operative().to_string(),
        price: terminal.price().to_string(),
        pay_token: terminal.pay_token().to_string(),
        payment_processor: terminal.payment_processor().map(str::to_string),
        published_at: crate::auth::now_ts(),
    };
    if let Some(existing) = load_runtime_custody_listing(data_dir, mint_id)? {
        if existing.schema == expected.schema
            && existing.mint_id == expected.mint_id
            && existing.content_id == expected.content_id
            && existing.content_access_id == expected.content_access_id
            && existing.cid == expected.cid
            && existing.metadata_cid == expected.metadata_cid
            && existing.token_uri == expected.token_uri
            && existing.publisher_principal_id == expected.publisher_principal_id
            && existing
                .seller_address
                .eq_ignore_ascii_case(&expected.seller_address)
            && existing.chain_namespace == expected.chain_namespace
            && existing.network == expected.network
            && existing.ledger.eq_ignore_ascii_case(&expected.ledger)
            && existing.token_id.eq_ignore_ascii_case(&expected.token_id)
            && existing.operative.eq_ignore_ascii_case(&expected.operative)
            && existing.price == expected.price
            && existing.pay_token.eq_ignore_ascii_case(&expected.pay_token)
            && existing
                .payment_processor
                .as_deref()
                .map(str::to_ascii_lowercase)
                == expected
                    .payment_processor
                    .as_deref()
                    .map(str::to_ascii_lowercase)
        {
            return Ok(());
        }
        anyhow::bail!("Runtime custody listing is invalid");
    }
    persist_runtime_custody_listing(data_dir, &expected)
}

pub(crate) fn load_runtime_custody_purchase(
    data_dir: &Path,
    principal_id: &str,
    mint_id: Digest32,
) -> anyhow::Result<Option<RuntimeCustodyPurchaseRecord>> {
    let path = runtime_purchase_path(data_dir, principal_id, mint_id);
    if !path.exists() {
        return Ok(None);
    }
    let record: RuntimeCustodyPurchaseRecord = serde_json::from_slice(&fs::read(path)?)?;
    if record.schema != RUNTIME_PURCHASE_SCHEMA_V1 || record.principal_id != principal_id {
        anyhow::bail!("Runtime custody purchase is invalid");
    }
    Ok(Some(record))
}

pub(crate) fn persist_runtime_custody_purchase(
    data_dir: &Path,
    purchase: &RuntimeCustodyPurchaseRecord,
) -> anyhow::Result<()> {
    write_owner_only_bytes(
        &runtime_purchase_path(
            data_dir,
            &purchase.principal_id,
            parse_mint_id_hex(&purchase.mint_id)?,
        ),
        &serde_json::to_vec(purchase)?,
    )
}

fn load_runtime_custody_viewer_record(
    data_dir: &Path,
    principal_id: &str,
    mint_id: Digest32,
) -> anyhow::Result<Option<RuntimeCustodyViewerRecord>> {
    let path = runtime_viewer_path(data_dir, principal_id, mint_id);
    let Some(bytes) = load_runtime_custody_viewer_record_bytes(&path)? else {
        return Ok(None);
    };
    let record: RuntimeCustodyViewerRecord = serde_json::from_slice(&bytes)?;
    Ok(Some(record))
}

fn load_runtime_custody_viewer_record_from_path(
    path: &Path,
) -> anyhow::Result<Option<RuntimeCustodyViewerRecord>> {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    if extension != "json" {
        return Ok(None);
    }
    let Some(bytes) = load_runtime_custody_viewer_record_bytes(path)? else {
        return Ok(None);
    };
    let record: RuntimeCustodyViewerRecord = serde_json::from_slice(&bytes)?;
    Ok(Some(record))
}

fn load_runtime_custody_viewer_record_bytes(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut reader = file.take(MAX_RUNTIME_VIEWER_RECORD_BYTES + 1);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RUNTIME_VIEWER_RECORD_BYTES {
        anyhow::bail!("Runtime custody viewer session is unavailable");
    }
    Ok(Some(bytes))
}

fn validate_runtime_custody_viewer_record_identity(
    data_dir: &Path,
    path: &Path,
    record: &RuntimeCustodyViewerRecord,
) -> anyhow::Result<Digest32> {
    if record.schema != RUNTIME_VIEWER_SCHEMA_V1 {
        anyhow::bail!("viewer record schema is invalid");
    }
    let mint_id = parse_mint_id_hex(&record.mint_id)?;
    if runtime_viewer_path(data_dir, &record.principal_id, mint_id) != path {
        anyhow::bail!("viewer record path does not match its identities");
    }
    let purchase = load_runtime_custody_purchase(data_dir, &record.principal_id, mint_id)?
        .ok_or_else(|| anyhow::anyhow!("viewer record purchase is unavailable"))?;
    if purchase.principal_id != record.principal_id
        || purchase.profile_did != record.profile_did
        || purchase.content_id != record.content_id
    {
        anyhow::bail!("viewer record identities do not match the durable purchase");
    }
    let mint = runtime_mint_journal(data_dir)
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!("viewer record mint is unavailable"))?;
    if runtime_protected_content_id(mint.draft().encrypted_content())? != record.content_id {
        anyhow::bail!("viewer record content does not match the durable mint");
    }
    let _ = record.audit_request_id()?;
    let _ = record.viewer_session_handle_bytes()?;
    if record.expires_at == 0 {
        anyhow::bail!("viewer record expiry is invalid");
    }
    if record.lifecycle_status != RuntimeCustodyViewerLifecycleStatus::OpenPending {
        let _ = record.to_runtime_viewer_session(&mint)?;
    }
    Ok(mint_id)
}

fn persist_runtime_custody_viewer_record(
    data_dir: &Path,
    principal_id: &str,
    mint_id: Digest32,
    record: &RuntimeCustodyViewerRecord,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RUNTIME_VIEWER_RECORD_BYTES {
        anyhow::bail!("Runtime custody viewer session is unavailable");
    }
    write_owner_only_bytes(
        &runtime_viewer_path(data_dir, principal_id, mint_id),
        &bytes,
    )
}

async fn settle_runtime_custody_viewer_cleanup(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    mint_id: Digest32,
    mut record: RuntimeCustodyViewerRecord,
) -> anyhow::Result<RuntimeCustodyViewerRecord> {
    let mint = runtime_mint_journal(data_dir)
        .load(mint_id)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint selection is invalid"))?;
    let decrypt = RuntimeDecryptRegistryAdapter::new(registry);
    match record.lifecycle_status {
        RuntimeCustodyViewerLifecycleStatus::OpenPending => {
            let session = record.to_runtime_viewer_session(&mint)?;
            let close_result = match record.open_pending_close_result() {
                Some(RuntimeCustodyOpenPendingCloseResult::Closed) => {
                    RuntimeViewerSessionCloseResult::Closed
                }
                Some(RuntimeCustodyOpenPendingCloseResult::AlreadyAbsent) => {
                    RuntimeViewerSessionCloseResult::AlreadyAbsent
                }
                None => {
                    let result = close_viewer_session_with_result(&decrypt, &session)
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE)
                        })?;
                    record.mark_open_pending_close_result(result, crate::auth::now_ts());
                    persist_runtime_custody_viewer_record(
                        data_dir,
                        principal_id,
                        mint_id,
                        &record,
                    )?;
                    result
                }
            };
            let cancel_result = match record.open_pending_cancel_result() {
                Some(RuntimeCustodyOpenPendingCancelResult::Cancelled) => {
                    RuntimePreparedRecipientCancelResult::Cancelled
                }
                Some(RuntimeCustodyOpenPendingCancelResult::AlreadyAbsent) => {
                    RuntimePreparedRecipientCancelResult::AlreadyAbsent
                }
                None => {
                    let result = cancel_prepared_recipient_with_result_by_handle(
                        &decrypt,
                        record.audit_request_id()?,
                        record.viewer_session_handle_bytes()?,
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE))?;
                    record.mark_open_pending_cancel_result(result, crate::auth::now_ts());
                    persist_runtime_custody_viewer_record(
                        data_dir,
                        principal_id,
                        mint_id,
                        &record,
                    )?;
                    result
                }
            };
            let _ = cancel_result;
            record.mark_terminal(close_result, crate::auth::now_ts());
            persist_runtime_custody_viewer_record(data_dir, principal_id, mint_id, &record)?;
            Ok(record)
        }
        RuntimeCustodyViewerLifecycleStatus::Active
        | RuntimeCustodyViewerLifecycleStatus::CleanupPending => {
            let session = record.to_runtime_viewer_session(&mint)?;
            match close_viewer_session_with_result(&decrypt, &session).await {
                Ok(result) => {
                    record.mark_terminal(result, crate::auth::now_ts());
                    persist_runtime_custody_viewer_record(
                        data_dir,
                        principal_id,
                        mint_id,
                        &record,
                    )?;
                    Ok(record)
                }
                Err(_) => {
                    record.mark_cleanup_pending(crate::auth::now_ts());
                    persist_runtime_custody_viewer_record(
                        data_dir,
                        principal_id,
                        mint_id,
                        &record,
                    )?;
                    anyhow::bail!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE);
                }
            }
        }
        RuntimeCustodyViewerLifecycleStatus::Closed
        | RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent => Ok(record),
    }
}

/// Reconciles durable protected-content viewer records after canonical decrypt
/// provider registration so restart cleanup can settle exact viewer sessions.
pub async fn reconcile_runtime_custody_viewers_after_decrypt_registration(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
) -> anyhow::Result<()> {
    let root = data_dir.join(RUNTIME_OPEN_MATERIAL_ROOT);
    if !root.is_dir() {
        return Ok(());
    }
    let mut unresolved_mint_dirs = 0usize;
    let mut unresolved_viewer_records = 0usize;
    for mint_entry in fs::read_dir(&root)? {
        let mint_entry = mint_entry?;
        if !mint_entry.file_type()?.is_dir() {
            continue;
        }
        let viewers_dir = mint_entry.path().join("viewers");
        if !viewers_dir.is_dir() {
            continue;
        }
        let mut counted_mint_dir = false;
        for viewer_entry in fs::read_dir(&viewers_dir)? {
            let viewer_entry = viewer_entry?;
            if !viewer_entry.file_type()?.is_file() {
                continue;
            }
            let path = viewer_entry.path();
            let _viewer_lifecycle_guard =
                acquire_runtime_custody_viewer_lifecycle_guard_for_path(&path).await;
            let Some(record) = load_runtime_custody_viewer_record_from_path(&path)? else {
                continue;
            };
            let lifecycle_status = record.lifecycle_status;
            if !matches!(
                record.lifecycle_status,
                RuntimeCustodyViewerLifecycleStatus::OpenPending
                    | RuntimeCustodyViewerLifecycleStatus::Active
                    | RuntimeCustodyViewerLifecycleStatus::CleanupPending
            ) {
                if validate_runtime_custody_viewer_record_identity(data_dir, &path, &record)
                    .is_err()
                {
                    continue;
                }
                continue;
            }
            if !counted_mint_dir {
                unresolved_mint_dirs += 1;
                counted_mint_dir = true;
                if unresolved_mint_dirs > MAX_RUNTIME_VIEWER_RECONCILE_MINT_DIRS {
                    anyhow::bail!(
                        "Runtime custody viewer reconciliation exceeds unresolved mint directory limit"
                    );
                }
            }
            unresolved_viewer_records += 1;
            if unresolved_viewer_records > MAX_RUNTIME_VIEWER_RECONCILE_RECORDS {
                anyhow::bail!(
                    "Runtime custody viewer reconciliation exceeds unresolved record limit"
                );
            }
            let mint_id = validate_runtime_custody_viewer_record_identity(data_dir, &path, &record)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "Runtime custody viewer reconciliation found invalid {:?} record at {}: {}",
                        lifecycle_status,
                        path.display(),
                        error
                    )
                })?;
            let principal_id = record.principal_id.clone();
            if let Err(error) = settle_runtime_custody_viewer_cleanup(
                data_dir,
                registry.clone(),
                &principal_id,
                mint_id,
                record,
            )
            .await
            {
                tracing::warn!(
                    "Runtime custody viewer cleanup reconciliation remains pending at {}: {}",
                    path.display(),
                    error
                );
            }
        }
    }
    Ok(())
}

fn reconstructed_buy_receipt(
    mint: &elastos_protected_content_runtime::PersistedRuntimeMint,
    purchase: &RuntimeCustodyPurchaseRecord,
    current_profile_did: &str,
) -> anyhow::Result<elastos_protected_content_runtime::RuntimeBuyReceipt> {
    if purchase.profile_did != current_profile_did {
        return Err(runtime_open_error(
            elastos_protected_content_runtime::RuntimeOpenError::ChainEvidence,
        ));
    }
    let profile_identity = profile_identity_from_did(current_profile_did)?;
    let RuntimeCustodyPurchaseProgress::Complete { terminal } = &purchase.progress else {
        return Err(runtime_open_error(
            elastos_protected_content_runtime::RuntimeOpenError::ChainEvidence,
        ));
    };
    let effect = RuntimeVerifiedPurchaseEffect::new(
        RuntimeProtectedContentPurchaseIntent::new(
            mint.draft().mint_id(),
            mint.draft().encrypted_content().clone(),
            mint.draft().key_envelope().clone(),
            mint.draft().policy().clone(),
            RightsActionV1::View,
            purchase.chain_namespace.clone(),
            purchase.network.clone(),
            purchase.buy_stage.to.clone(),
            purchase.buy_stage.value.clone(),
            purchase.buy_stage.data.clone(),
        )
        .map_err(|_| anyhow::anyhow!("Runtime custody chain evidence is invalid"))?,
        RuntimePurchaseEffectAuthority::new(
            purchase.principal_id.clone(),
            purchase.account_id.clone(),
            purchase.address.clone(),
            purchase.buy_stage.approval_request_id.clone(),
        )
        .map_err(|_| anyhow::anyhow!("Runtime custody wallet authority is invalid"))?,
        terminal.wallet_binding.clone(),
        terminal.chain_transaction.clone(),
        terminal.chain_observation.clone(),
        terminal.confirmed_at,
    )
    .map_err(runtime_open_error)?;
    bind_buy(mint, &purchase.principal_id, profile_identity, &effect).map_err(runtime_open_error)
}

struct RuntimeReleaseWalletInvocation<'a> {
    principal_id: &'a str,
    account_id: &'a str,
    proof_binding_id: &'a str,
    session_id: &'a str,
    grant_id: &'a str,
    mint_id: Digest32,
    runtime_session_binding: RuntimeSessionBindingV1,
}

async fn invoke_runtime_release_wallet(
    registry: &ProviderRegistry,
    buy: &elastos_protected_content_runtime::RuntimeBuyReceipt,
    recipient: &RecipientKeyIdentityV1,
    invocation: RuntimeReleaseWalletInvocation<'_>,
    now: u64,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, WalletSignedRightsRequestV1)> {
    let mut replay_nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut replay_nonce);
    let rights_request = RightsRequestV1::new(
        buy.binding_for_session(invocation.runtime_session_binding)
            .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?,
        buy.action(),
        recipient.clone(),
        now.saturating_sub(5),
        now + 180,
        ReplayNonce16::new(replay_nonce),
    )
    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let mut request_entropy = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut request_entropy);
    let context = VerifiedWalletInvocationContext::new(
        invocation.principal_id.to_string(),
        invocation.session_id,
        Some(invocation.proof_binding_id.to_string()),
        invocation.grant_id,
        "runtime",
        format!(
            "runtime-open-{}",
            hex::encode(invocation.mint_id.as_bytes())
        ),
    )
    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let wallet_request = WalletProviderRequestV2::new(
        &context,
        format!("wallet-request:{}", hex::encode(request_entropy)),
        now,
        now.saturating_add(MAX_INVOCATION_TTL_SECS),
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            account_id: invocation.account_id.to_string(),
            canonical_rights_request_hex: hex::encode(rights_request.canonical_bytes().map_err(
                |_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE),
            )?),
            reason: "Open protected content".to_string(),
        },
    )
    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: RUNTIME_PROVIDER_ID.to_string(),
            target: WALLET_PROVIDER_ID.to_string(),
            op: WALLET_BUS_OPERATION.to_string(),
            request: serde_json::json!({
                "op": WALLET_BUS_OPERATION,
                "request": wallet_request,
            }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
    }
    let data = response
        .get("data")
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let request_bytes = serde_json::to_vec(&wallet_request)
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let response_bytes = serde_json::to_vec(data)
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let signed_rights = wallet_signed_rights_from_bytes(&request_bytes, &response_bytes)
        .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    if signed_rights.request() != &rights_request {
        anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
    }
    Ok((request_bytes, response_bytes, signed_rights))
}

fn wallet_signed_rights_from_bytes(
    wallet_request_bytes: &[u8],
    wallet_response_bytes: &[u8],
) -> Option<WalletSignedRightsRequestV1> {
    let now = crate::auth::now_ts();
    let request =
        elastos_wallet_contract::WalletProviderRequestV2::decode_at(wallet_request_bytes, now)
            .ok()?;
    let response = elastos_wallet_contract::WalletProviderResponseV2::decode_for_request(
        wallet_response_bytes,
        &request,
    )
    .ok()?;
    let data = match response.result {
        elastos_wallet_contract::WalletResultV2::Ok { data } => data,
        _ => return None,
    };
    let hex_value = data.get("wallet_signed_rights_request_hex")?.as_str()?;
    let bytes = decode_hex_bytes(hex_value).ok()?;
    WalletSignedRightsRequestV1::from_canonical_bytes(&bytes).ok()
}

fn sign_runtime_terminal_receipt(
    device_key: &ed25519_dalek::SigningKey,
    operation: &SignedRuntimeReleaseOperationV1,
    contributions: &[elastos_protected_content_contracts::SignedNodeContributionV1],
    epoch: &SignedCustodyEpochV1,
    now: u64,
) -> anyhow::Result<SignedTerminalReceiptV1> {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), now)
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let node_set = epoch
        .statement()
        .node_set()
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let refs = contributions
        .iter()
        .map(|contribution| {
            authenticated
                .verify_node_contribution(contribution, &node_set, now)
                .map(|verified| NodeContributionRefV1::from(&verified))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    let contribution_issued_at = contributions
        .iter()
        .map(|contribution| contribution.statement().issued_at())
        .max()
        .unwrap_or(now);
    let contribution_expires_at = contributions
        .iter()
        .map(|contribution| contribution.statement().expires_at())
        .min()
        .unwrap_or(now.saturating_add(30));
    let release = operation.statement().release_request();
    let issued_at = now.max(contribution_issued_at).max(release.issued_at());
    let expires_at = contribution_expires_at
        .min(release.expires_at())
        .min(issued_at.saturating_add(30));
    if expires_at <= issued_at {
        anyhow::bail!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE);
    }
    let statement = TerminalReceiptStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.binding().clone(),
        TerminalReceiptIssuerKey::new(device_key.verifying_key().to_bytes())
            .map_err(|_| anyhow::anyhow!("local Runtime device signing key is invalid"))?,
        KeyReleaseOutcomeV1::Released,
        refs,
        issued_at,
        expires_at,
    )
    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    SignedTerminalReceiptV1::new(
        statement.clone(),
        device_key
            .sign(&statement.canonical_bytes().map_err(|_| {
                anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE)
            })?)
            .to_bytes()
            .to_vec(),
    )
    .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))
}

async fn fetch_runtime_custody_open_media(
    registry: &ProviderRegistry,
    cid: &str,
    expected: &CencFmp4MediaIdentityV1,
) -> anyhow::Result<(CencFmp4MediaIdentityV1, Vec<u8>)> {
    let identity_bytes = crate::content::fetch_bytes_via_provider(
        registry,
        cid,
        Some(PROTECTED_CONTENT_IDENTITY_PATH),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    let media = CencFmp4MediaIdentityV1::from_canonical_bytes(&identity_bytes)
        .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    if &media != expected {
        anyhow::bail!("Runtime custody content availability is unavailable");
    }
    let init =
        crate::content::fetch_bytes_via_provider(registry, cid, Some(PROTECTED_CONTENT_INIT_PATH))
            .await
            .map_err(|_| anyhow::anyhow!("Runtime custody content availability is unavailable"))?;
    Ok((media, init))
}

fn runtime_open_error(error: elastos_protected_content_runtime::RuntimeOpenError) -> anyhow::Error {
    match error {
        elastos_protected_content_runtime::RuntimeOpenError::WalletAuthority => {
            anyhow::anyhow!("Runtime custody wallet authority is invalid")
        }
        elastos_protected_content_runtime::RuntimeOpenError::MintSelection => {
            anyhow::anyhow!("Runtime custody mint selection is invalid")
        }
        elastos_protected_content_runtime::RuntimeOpenError::DecryptResult => {
            anyhow::anyhow!(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE)
        }
        _ => anyhow::anyhow!("Runtime custody chain evidence is invalid"),
    }
}

fn parse_mint_id_hex(value: &str) -> anyhow::Result<Digest32> {
    let bytes = decode_hex_bytes(value)
        .map_err(|_| anyhow::anyhow!("Runtime custody mint identity is invalid"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Runtime custody mint identity is invalid"))?;
    Ok(Digest32::new(bytes))
}

fn parse_viewer_handle_hex(
    value: &str,
) -> anyhow::Result<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]> {
    let bytes = decode_hex_bytes(value)
        .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Runtime custody viewer session is unavailable"))
}

fn decode_hex_bytes(value: &str) -> Result<Vec<u8>, hex::FromHexError> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
}

fn runtime_open_audit_id(
    principal_id: &str,
    mint_id: Digest32,
    now: u64,
) -> RuntimeReleaseAuditIdV1 {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"elastos.protected-content.runtime-open-audit.v1");
    hasher.update(principal_id.as_bytes());
    hasher.update(mint_id.as_bytes());
    hasher.update(now.to_be_bytes());
    RuntimeReleaseAuditIdV1::new(Digest32::new(hasher.finalize().into())).unwrap_or_else(|_| {
        RuntimeReleaseAuditIdV1::new(Digest32::new([1; 32])).expect("nonzero audit id")
    })
}

fn load_runtime_custody_profile_did(data_dir: &Path, principal_id: &str) -> anyhow::Result<String> {
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let profile = crate::collaboration_profile_authority::load_profile_authority(
        data_dir,
        principal_id,
        &localhost_root,
    )?
    .ok_or_else(|| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    Ok(profile.document().profile_did.clone())
}

fn profile_identity_from_did(profile_did: &str) -> anyhow::Result<ProfileIdentityV1> {
    let key = crate::crypto::decode_did_key(profile_did)
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    ProfileIdentityV1::from_public_key_bytes(key.to_bytes())
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))
}

fn update_length_prefixed_session_field(
    hasher: &mut sha2::Sha256,
    value: &[u8],
) -> anyhow::Result<()> {
    let encoded_len = u32::try_from(value.len())
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))?;
    hasher.update(encoded_len.to_be_bytes());
    hasher.update(value);
    Ok(())
}

fn derive_runtime_custody_session_binding(
    principal_id: &str,
    profile_did: &str,
    proof_binding_id: &str,
    session_id: &str,
    grant_id: &str,
    mint_id: Digest32,
) -> anyhow::Result<RuntimeSessionBindingV1> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"elastos.protected-content.runtime-session-binding.v1");
    update_length_prefixed_session_field(&mut hasher, principal_id.as_bytes())?;
    update_length_prefixed_session_field(&mut hasher, profile_did.as_bytes())?;
    update_length_prefixed_session_field(&mut hasher, proof_binding_id.as_bytes())?;
    update_length_prefixed_session_field(&mut hasher, session_id.as_bytes())?;
    update_length_prefixed_session_field(&mut hasher, grant_id.as_bytes())?;
    update_length_prefixed_session_field(&mut hasher, mint_id.as_bytes())?;
    RuntimeSessionBindingV1::new(Digest32::new(hasher.finalize().into()))
        .map_err(|_| anyhow::anyhow!(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE))
}

fn audit_nonce_bytes(audit_request_id: RuntimeReleaseAuditIdV1) -> [u8; 16] {
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(&audit_request_id.digest().as_bytes()[..16]);
    nonce
}

fn runtime_open_envelope_path(data_dir: &Path, mint_id: Digest32) -> PathBuf {
    data_dir
        .join(RUNTIME_OPEN_MATERIAL_ROOT)
        .join(hex::encode(mint_id.as_bytes()))
        .join("envelope.bin")
}

fn runtime_listing_path(data_dir: &Path, mint_id: Digest32) -> PathBuf {
    data_dir
        .join(RUNTIME_LISTING_ROOT)
        .join(format!("{}.json", hex::encode(mint_id.as_bytes())))
}

fn runtime_purchase_path(data_dir: &Path, principal_id: &str, mint_id: Digest32) -> PathBuf {
    let mut hasher = sha2::Sha256::new();
    hasher.update(principal_id.as_bytes());
    data_dir
        .join(RUNTIME_PURCHASE_ROOT)
        .join(hex::encode(hasher.finalize()))
        .join(format!("{}.json", hex::encode(mint_id.as_bytes())))
}

fn runtime_viewer_path(data_dir: &Path, principal_id: &str, mint_id: Digest32) -> PathBuf {
    let mut hasher = sha2::Sha256::new();
    hasher.update(principal_id.as_bytes());
    data_dir
        .join(RUNTIME_OPEN_MATERIAL_ROOT)
        .join(hex::encode(mint_id.as_bytes()))
        .join("viewers")
        .join(format!("{}.json", hex::encode(hasher.finalize())))
}

fn runtime_storage_write_error(reason: String) -> anyhow::Error {
    anyhow::anyhow!("Runtime custody protected-content storage is unavailable: {reason}")
}

#[cfg(unix)]
fn ensure_owner_only_runtime_storage_parent(path: &Path) -> anyhow::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = path;
    loop {
        if cursor.exists() {
            let metadata = fs::symlink_metadata(cursor).map_err(|_| {
                runtime_storage_write_error("parent path is unavailable".to_string())
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                anyhow::bail!(
                    "{}",
                    runtime_storage_write_error(
                        "parent path must be an owner-only directory".to_string()
                    )
                );
            }
            validate_owner_only_metadata_with_error(
                "protected-content runtime storage parent",
                &metadata,
                false,
                runtime_storage_write_error,
            )?;
            break;
        }
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or_else(|| runtime_storage_write_error("parent path is unavailable".to_string()))?;
    }
    for dir in missing.iter().rev() {
        fs::create_dir(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    for dir in missing {
        let metadata = fs::symlink_metadata(&dir)
            .map_err(|_| runtime_storage_write_error("parent path is unavailable".to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            anyhow::bail!(
                "{}",
                runtime_storage_write_error(
                    "parent path must be an owner-only directory".to_string()
                )
            );
        }
        validate_owner_only_metadata_with_error(
            "protected-content runtime storage parent",
            &metadata,
            false,
            runtime_storage_write_error,
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_runtime_storage_parent(parent: &Path) -> anyhow::Result<()> {
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn runtime_storage_temp_path(path: &Path) -> anyhow::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| runtime_storage_write_error("parent path is unavailable".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| runtime_storage_write_error("storage path is invalid".to_string()))?;
    for _ in 0..16 {
        let mut nonce = [0u8; 8];
        rand::thread_rng().fill_bytes(&mut nonce);
        let candidate = parent.join(format!(
            ".{}.tmp-{}",
            file_name.to_string_lossy(),
            hex::encode(nonce)
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "{}",
        runtime_storage_write_error("temporary path allocation failed".to_string())
    );
}

fn write_owner_only_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        #[cfg(unix)]
        ensure_owner_only_runtime_storage_parent(parent)?;
        #[cfg(not(unix))]
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        let temp_path = runtime_storage_temp_path(path)?;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true).mode(0o600);
        let mut file = options.open(&temp_path)?;
        let result = (|| -> anyhow::Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp_path, path)?;
            if let Some(parent) = path.parent() {
                sync_runtime_storage_parent(parent)?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result?;
    }
    #[cfg(not(unix))]
    {
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, bytes)?;
        fs::rename(temp_path, path)?;
    }
    Ok(())
}

fn runtime_viewer_lifecycle_guards(
) -> &'static StdMutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>> {
    RUNTIME_VIEWER_LIFECYCLE_GUARDS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn runtime_viewer_lifecycle_guard_key(
    data_dir: &Path,
    principal_id: &str,
    mint_id: Digest32,
) -> PathBuf {
    runtime_viewer_lifecycle_guard_key_for_path(&runtime_viewer_path(
        data_dir,
        principal_id,
        mint_id,
    ))
}

fn runtime_viewer_lifecycle_guard_key_for_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn runtime_viewer_lifecycle_lock_by_key(key: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let mut guards = match runtime_viewer_lifecycle_guards().lock() {
        Ok(guards) => guards,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(lock) = guards.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    guards.insert(key.to_path_buf(), Arc::downgrade(&lock));
    lock
}

struct RuntimeViewerLifecycleExecutionGuard {
    key: PathBuf,
    lock: Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for RuntimeViewerLifecycleExecutionGuard {
    fn drop(&mut self) {
        if Arc::strong_count(&self.lock) > 2 {
            return;
        }
        let mut guards = match runtime_viewer_lifecycle_guards().lock() {
            Ok(guards) => guards,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guards
            .get(&self.key)
            .and_then(Weak::upgrade)
            .is_some_and(|lock| Arc::ptr_eq(&lock, &self.lock))
        {
            guards.remove(&self.key);
        }
    }
}

async fn acquire_runtime_custody_viewer_lifecycle_guard(
    data_dir: &Path,
    principal_id: &str,
    mint_id: Digest32,
) -> RuntimeViewerLifecycleExecutionGuard {
    let key = runtime_viewer_lifecycle_guard_key(data_dir, principal_id, mint_id);
    acquire_runtime_custody_viewer_lifecycle_guard_by_key(key).await
}

async fn acquire_runtime_custody_viewer_lifecycle_guard_for_path(
    path: &Path,
) -> RuntimeViewerLifecycleExecutionGuard {
    acquire_runtime_custody_viewer_lifecycle_guard_by_key(
        runtime_viewer_lifecycle_guard_key_for_path(path),
    )
    .await
}

async fn acquire_runtime_custody_viewer_lifecycle_guard_by_key(
    key: PathBuf,
) -> RuntimeViewerLifecycleExecutionGuard {
    let lock = runtime_viewer_lifecycle_lock_by_key(&key);
    let guard = lock.clone().lock_owned().await;
    RuntimeViewerLifecycleExecutionGuard {
        key,
        lock,
        _guard: guard,
    }
}
