use std::collections::{BTreeMap, VecDeque};
use std::env;
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use base64::Engine as _;
use custody_provider::{
    parse_and_verify_provisioning_output, ProvisionedCustodyProviderPublicKeys,
};
use ed25519_dalek::{Signer as _, SigningKey};
use elastos_protected_content_custody::provision_custody_envelope;
use elastos_protected_content_rights::{
    chain_rights_evidence_request, CHAIN_PROVIDER_ID, CHAIN_RIGHTS_EVIDENCE_OP,
};
use elastos_protected_content_runtime::{
    bind_buy, cancel_prepared_recipient, close_viewer_session, open_viewer_session,
    prepare_recipient, read_viewer_media_part, resolve_runtime_mint_selected_nodes,
    PersistedRuntimeMint, RuntimeContentAvailabilityRequirement, RuntimeCustodyProvider,
    RuntimeDecryptProvider, RuntimeMediaPreparationRecord, RuntimeMediaPreparationState,
    RuntimeMintCoordinator, RuntimeMintCoordinatorOutcome, RuntimeMintDraft, RuntimeMintIntent,
    RuntimeMintJournal, RuntimeMintNodeBinding, RuntimeMintNodeReceipt, RuntimeMintSelectedNode,
    RuntimeOpenError, RuntimeOpenViewerSessionInput, RuntimeProtectedContentPurchaseIntent,
    RuntimeProviderCallError, RuntimePurchaseEffectAuthority, RuntimeReleaseCoordinator,
    RuntimeReleaseCoordinatorOutcome, RuntimeReleaseJournal, RuntimeReleaseTerminalResult,
    RuntimeRightsProvider, RuntimeSelectedProvider, RuntimeVerifiedPurchaseEffect,
    RuntimeViewerSession,
};
use elastos_runtime::provider::{
    bridge::ProviderConfig, CapsuleProvider, Provider, ProviderBridge, ProviderCarrierInvoker,
    ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer, ResourceRequest, ResourceResponse,
};
use elastos_wallet_contract::{
    ProtectedContentRightsSignatureResultV1, ValidatedChainOutcomeBindingV1,
    VerifiedWalletInvocationContext, WalletProviderOperationV2, WalletProviderRequestV2,
    WalletProviderResponseV2, WalletResultV2,
};
use k256::ecdsa::SigningKey as WalletSigningKey;
use serde_json::{json, Value};
use sha2::Digest as _;
use sha3::Keccak256;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify};
use x_wing::kem::{Decapsulator as _, KeyExport as _};
use x_wing::TryKeyInit as _;

use super::{
    invoke_json_provider, list_unresolved_runtime_releases, load_or_persist_runtime_mint_intent,
    load_runtime_custody_composition, load_runtime_custody_composition_config,
    load_runtime_media_provider_bridge_config,
    load_runtime_protected_content_chain_provider_config, prepare_runtime_custody_library_source,
    prepare_runtime_media_provider_prerequisite_with_path, publish_runtime_custody_library_object,
    publish_runtime_custody_library_source, register_inactive_custody_provider,
    register_inactive_custody_runtime_provider_target, register_protect_provider,
    register_protected_content_decrypt_provider, resolve_runtime_rights_policy,
    runtime_mint_journal, runtime_protected_content_id, runtime_purchase_path, source_media_digest,
    unresolved_release_audit_records, write_owner_only_bytes, InactiveCustodyProvider,
    RuntimeCustodyComposition, RuntimeCustodyCompositionConfigFile,
    RuntimeCustodyLibraryPublishInput, RuntimeCustodyLibrarySourceInput,
    RuntimeCustodyPurchaseAccessEvidenceRecord, RuntimeCustodyPurchaseProgress,
    RuntimeCustodyPurchaseRecord, RuntimeCustodyPurchaseStageRecord, RuntimeCustodyRegistryAdapter,
    RuntimeCustodyRouteBindingConfig, RuntimeCustodyRouteTransportConfig,
    RuntimeCustodyTerminalPurchaseRecord, RuntimeDecryptRegistryAdapter,
    RuntimeLibraryMediaPreparation, CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1,
    CUSTODY_COMPOSITION_SCHEMA_V1, CUSTODY_PROVIDER_ID, CUSTODY_PROVIDER_OPERATIONS,
    CUSTODY_PROVIDER_VERSION, MAX_CHAIN_PROVIDER_CONFIG_BYTES, MEDIA_PROVIDER_CONFIG_SCHEMA_V1,
    MEDIA_PROVIDER_ID, MEDIA_PROVIDER_MAX_INPUT_BYTES_V1, MEDIA_PROVIDER_TIMEOUT_MS_V1,
    PROTECTED_CONTENT_CHAIN_PROVIDER_CONFIG_SCHEMA_V1, PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
    PROTECTED_CONTENT_DECRYPT_PROVIDER_OPERATIONS, PROTECTED_CONTENT_DECRYPT_PROVIDER_VERSION,
    PROTECTED_CONTENT_PROVIDER_STATUS_TIMEOUT, PROTECT_PROVIDER_ID, PROTECT_PROVIDER_OPERATIONS,
    PROTECT_PROVIDER_PROCESS_ID, PROTECT_PROVIDER_VERSION,
    RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE, RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE,
    RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE,
    RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE, RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE,
    RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE, RUNTIME_PROVIDER_ID,
    RUNTIME_PURCHASE_SCHEMA_V1,
};
use elastos_protected_content_contracts::{
    CanonicalContract, ContentAccessIdV1, CustodyApprovedSuitesV1,
    CustodyCommitteeAuthorizationIdentityV1, CustodyCommitteeAuthorizationStatementV1,
    CustodyEnvelopeManifestV1, CustodyEnvelopeV1, CustodyEpochIssuerKeyV1, CustodyEpochStatementV1,
    CustodyNodeProvisioningRecordIdentityV1, CustodyNodeProvisioningRecordV1,
    CustodyPoolFailureDomainIdV1, CustodyPoolIdentityV1, CustodyPoolMemberStateV1,
    CustodyPoolMemberV1, CustodyPoolOperatorIdV1, CustodyPoolStatementV1, Digest32,
    EncryptedContentIdentityV1, EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1,
    KeyReleaseOutcomeV1, KeyReleaseRequestV1, NodeContributionRefV1, NodeContributionStatementV1,
    NodeCustodyPublicKeyV1, NodePublicKey, PqHybridSealedShareV1, ProfileIdentityV1,
    ProtectedContentBindingV1, RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1,
    RecipientPublicKeyBytesV1, RecipientSealedContributionV1, ReplayNonce16, RightsActionV1,
    RightsDecisionV1, RightsEvaluationEvidenceRequestV1, RightsEvaluationEvidenceV1,
    RightsObservationFinalityV1, RightsPolicyBodyV1, RightsRequestV1,
    RuntimeCustodyProvisioningIdV1, RuntimeCustodyProvisioningStatementV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
    RuntimeSessionBindingV1, ShareCoordinateV1, SignedCustodyCommitteeAuthorizationV1,
    SignedCustodyEpochV1, SignedCustodyPoolV1, SignedNodeContributionV1,
    SignedNodeRightsDecisionV1, SignedRecipientKeyAuthorizationV1,
    SignedRuntimeCustodyProvisioningV1, SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1,
    TerminalReceiptIssuerKey, TerminalReceiptStatementV1, ThresholdV1, ValidatedCustodyCommitteeV1,
    WalletAddress, WalletSignedRightsRequestV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    MAX_RIGHTS_EVIDENCE_LIFETIME_SECS, PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES,
    X_WING_DRAFT06_CIPHERTEXT_BYTES,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, CustodyProviderRequestV1, CustodyProviderResponseV1,
    DecryptProviderRequestV1, DecryptProviderResponseStatusV1, DecryptProviderResponseV1,
    ProtectProviderRequestV1, ProtectProviderResponseStatusV1, ProtectProviderResponseV1,
    ProtectionSessionNodeV1, ProviderFailureCodeV1, RightsProviderRequestV1,
    RightsProviderResponseV1, ValidatedClearFmp4MediaSessionLayoutV1,
    ValidatedCustodyProviderRequestV1, ValidatedDecryptProviderRequestV1,
    ValidatedRightsProviderRequestV1, ViewerMediaPartSelectorV1,
    CUSTODY_PROVIDER_REQUEST_SCHEMA_V1, CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1, DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
    MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1, PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
    PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
};

const TEST_VIEWER_LAUNCH_ID: &str = "launch:11111111111111111111111111111111";
const TEST_VIEWER_LAUNCH_ID_B: &str = "launch:22222222222222222222222222222222";

struct RecordingProvider {
    name: &'static str,
    requests: Mutex<Vec<Value>>,
    response: Value,
}

struct SequencedProvider {
    name: &'static str,
    requests: Mutex<Vec<Value>>,
    responses: Mutex<VecDeque<Result<Value, ProviderError>>>,
}

struct PrepareOnlyCleanupDecryptProvider {
    requests: Mutex<Vec<Value>>,
}

struct ProcessChainEvidenceProvider {
    expected_request: RightsProviderRequestV1,
    requests: Mutex<Vec<Value>>,
    has_access: bool,
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum TestMediaPreparationResponse {
    Prepared,
    SettledFailure,
    UnknownSettlement,
}

#[cfg(unix)]
struct TestMediaPreparationProvider {
    staging_root: PathBuf,
    requests: Mutex<Vec<String>>,
    response: TestMediaPreparationResponse,
}

#[cfg(unix)]
const TEST_CUSTODY_PROVIDER_BIN_ENV: &str = "ELASTOS_TEST_CUSTODY_PROVIDER_BIN";
#[cfg(unix)]
const TEST_DECRYPT_PROVIDER_BIN_ENV: &str = "ELASTOS_TEST_DECRYPT_PROVIDER_BIN";
#[cfg(unix)]
const TEST_PROTECT_PROVIDER_BIN_ENV: &str = "ELASTOS_TEST_PROTECT_PROVIDER_BIN";

#[derive(Clone)]
struct ContentAvailabilityTestConfig {
    policy: String,
    status: String,
    replicas: u32,
    live_multi_peer_proof: Option<bool>,
    checked_at: u64,
    receipt_cid: Option<String>,
    receipt_object_identity: Option<String>,
    receipt_publisher_did: Option<String>,
    malformed_receipt: bool,
    mutate_fetch_path: Option<String>,
    mutate_manifest_extra_file: bool,
}

impl ContentAvailabilityTestConfig {
    fn accepted() -> Self {
        Self {
            policy: "protected-content-replication/v1".to_string(),
            status: "network_available".to_string(),
            replicas: 3,
            live_multi_peer_proof: Some(true),
            checked_at: NOW,
            receipt_cid: None,
            receipt_object_identity: None,
            receipt_publisher_did: None,
            malformed_receipt: false,
            mutate_fetch_path: None,
            mutate_manifest_extra_file: false,
        }
    }

    fn accepted_now() -> Self {
        Self::accepted_at(crate::auth::now_ts())
    }

    fn accepted_at(checked_at: u64) -> Self {
        Self {
            checked_at,
            ..Self::accepted()
        }
    }
}

struct ContentAvailabilityTestProvider {
    signing_key: SigningKey,
    config: ContentAvailabilityTestConfig,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    manifest: Mutex<Option<crate::content::ContentObjectManifest>>,
    requests: Mutex<Vec<Value>>,
}

impl ContentAvailabilityTestProvider {
    const CID: &'static str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    fn new(seed: u8, config: ContentAvailabilityTestConfig) -> Arc<Self> {
        Self::with_signing_key(elastos_identity::derive_did(&[seed; 32]).0, config)
    }

    fn with_signing_key(
        signing_key: SigningKey,
        config: ContentAvailabilityTestConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            signing_key,
            config,
            files: Mutex::new(BTreeMap::new()),
            manifest: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn signer_did(&self) -> String {
        crate::crypto::domain_separated_sign(
            &self.signing_key,
            "elastos.content.availability.receipt.v1",
            b"content-availability-test-provider",
        )
        .1
    }

    async fn publish(&self, request: &Value) -> Result<Value, ProviderError> {
        let object_identity = request
            .get("object_did")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing object identity".to_string()))?;
        let publisher_did = request
            .get("publisher_did")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing publisher identity".to_string()))?;
        if request.get("object_kind").and_then(Value::as_str) != Some("protected-content") {
            return Err(ProviderError::Provider("wrong object kind".to_string()));
        }
        let entries = request
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Provider("missing files".to_string()))?;
        let mut stored = BTreeMap::new();
        let mut manifest_files = Vec::with_capacity(entries.len());
        for entry in entries {
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Provider("bad file path".to_string()))?;
            let bytes = entry
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Provider("missing file data".to_string()))
                .and_then(|encoded| {
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| ProviderError::Provider("bad file data".to_string()))
                })?;
            if stored.insert(path.to_string(), bytes.clone()).is_some() {
                return Err(ProviderError::Provider("duplicate file".to_string()));
            }
            manifest_files.push(crate::content::ContentObjectFile {
                path: path.to_string(),
                sha256: hex::encode(sha2::Sha256::digest(&bytes)),
                size: bytes.len() as u64,
            });
        }
        manifest_files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = crate::content::ContentObjectManifest {
            schema: "elastos.content.object.manifest/v1".to_string(),
            kind: "protected-content".to_string(),
            content_digest: super::content_object_digest(&manifest_files),
            files: manifest_files,
            links: Vec::new(),
            object_did: Some(object_identity.to_string()),
            publisher_did: Some(publisher_did.to_string()),
        };
        stored.insert(
            crate::content::CONTENT_OBJECT_MANIFEST_PATH.to_string(),
            serde_json::to_vec(&manifest)
                .map_err(|_| ProviderError::Provider("manifest encode failed".to_string()))?,
        );
        *self.files.lock().await = stored;
        *self.manifest.lock().await = Some(manifest);
        Ok(ok_provider_response(json!({ "cid": Self::CID })))
    }

    async fn status(&self) -> Result<Value, ProviderError> {
        if self.config.malformed_receipt {
            return Ok(ok_provider_response(json!({
                "receipt": {
                    "payload": "malformed",
                },
            })));
        }
        let manifest = self
            .manifest
            .lock()
            .await
            .clone()
            .ok_or_else(|| ProviderError::Provider("missing published object".to_string()))?;
        let object_identity = self
            .config
            .receipt_object_identity
            .as_deref()
            .or(manifest.object_did.as_deref())
            .ok_or_else(|| ProviderError::Provider("missing object identity".to_string()))?;
        let publisher_did = self
            .config
            .receipt_publisher_did
            .as_deref()
            .or(manifest.publisher_did.as_deref())
            .ok_or_else(|| ProviderError::Provider("missing publisher identity".to_string()))?;
        let cid = self.config.receipt_cid.as_deref().unwrap_or(Self::CID);
        let payload = crate::content::AvailabilityReceipt {
            schema: "elastos.content.availability.receipt/v1".to_string(),
            cid: cid.to_string(),
            uri: format!("elastos://{cid}"),
            object_did: Some(object_identity.to_string()),
            publisher_did: publisher_did.to_string(),
            provider: "content".to_string(),
            policy: self.config.policy.clone(),
            status: self.config.status.clone(),
            replicas: self.config.replicas,
            peer_selection: self
                .config
                .live_multi_peer_proof
                .map(|live_multi_peer_proof| {
                    json!({
                        "live_multi_peer_proof": live_multi_peer_proof,
                    })
                })
                .unwrap_or_else(|| json!({})),
            quota: json!({}),
            repair_worker: json!({}),
            storage_market: json!({}),
            repair_graph: json!({}),
            abuse_controls: json!({}),
            accounting: json!({}),
            checked_at: self.config.checked_at,
        };
        let payload_bytes = serde_json::to_string(
            &serde_json::to_value(&payload)
                .map_err(|_| ProviderError::Provider("receipt encode failed".to_string()))?,
        )
        .map_err(|_| ProviderError::Provider("receipt encode failed".to_string()))?;
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &self.signing_key,
            "elastos.content.availability.receipt.v1",
            payload_bytes.as_bytes(),
        );
        Ok(ok_provider_response(json!({
            "receipt": crate::content::SignedAvailabilityReceipt {
                payload,
                signature,
                signer_did,
            },
        })))
    }

    async fn fetch(&self, request: &Value) -> Result<Value, ProviderError> {
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing fetch path".to_string()))?;
        let mut bytes = if path == crate::content::CONTENT_OBJECT_MANIFEST_PATH
            && self.config.mutate_manifest_extra_file
        {
            let mut manifest = self
                .manifest
                .lock()
                .await
                .clone()
                .ok_or_else(|| ProviderError::Provider("missing manifest".to_string()))?;
            manifest.files.push(manifest.files[0].clone());
            serde_json::to_vec(&manifest)
                .map_err(|_| ProviderError::Provider("manifest encode failed".to_string()))?
        } else {
            self.files
                .lock()
                .await
                .get(path)
                .cloned()
                .ok_or_else(|| ProviderError::NotFound("content object file".to_string()))?
        };
        if self.config.mutate_fetch_path.as_deref() == Some(path) {
            let first = bytes
                .first_mut()
                .ok_or_else(|| ProviderError::Provider("empty fetch mutation".to_string()))?;
            *first ^= 0x01;
        }
        Ok(ok_provider_response(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        })))
    }

    async fn seed_published_protected_content(
        &self,
        directory: &Path,
        object_identity: &str,
        publisher_did: &str,
    ) -> Result<(), ProviderError> {
        let files = super::protected_content_directory_files(directory)
            .map_err(|error| ProviderError::Provider(error.to_string()))?
            .into_iter()
            .map(|path| {
                let bytes = fs::read(directory.join(&path))
                    .map_err(|error| ProviderError::Provider(error.to_string()))?;
                Ok(json!({
                    "path": path,
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                }))
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        self.publish(&json!({
            "op": "publish",
            "object_kind": "protected-content",
            "object_did": object_identity,
            "publisher_did": publisher_did,
            "files": files,
        }))
        .await?;
        Ok(())
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl Provider for ContentAvailabilityTestProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "content availability test provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["content"]
    }

    fn name(&self) -> &'static str {
        "content-availability-test-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        match request.get("op").and_then(Value::as_str) {
            Some("publish") => self.publish(request).await,
            Some("status") => self.status().await,
            Some("fetch") => self.fetch(request).await,
            _ => Err(ProviderError::Provider(
                "unsupported content operation".to_string(),
            )),
        }
    }
}

const NOW: u64 = 2_000_000_000;
const MEDIA_MIME_TYPE_V1: &str = "video/mp4";
const MEDIA_CODECS_V1: &str = "avc1.640028,mp4a.40.2";
const PQ_HYBRID_AEAD_NONCE_BYTES: usize = 12;
const PQ_HYBRID_WRAPPED_SHARE_BYTES: usize = 48;

impl RecordingProvider {
    fn new(name: &'static str, response: Value) -> Arc<Self> {
        Arc::new(Self {
            name,
            requests: Mutex::new(Vec::new()),
            response,
        })
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

impl SequencedProvider {
    fn new(name: &'static str, responses: Vec<Result<Value, ProviderError>>) -> Arc<Self> {
        Arc::new(Self {
            name,
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from(responses)),
        })
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

impl PrepareOnlyCleanupDecryptProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
        })
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

impl ProcessChainEvidenceProvider {
    fn new(expected_request: RightsProviderRequestV1, has_access: bool) -> Arc<Self> {
        Arc::new(Self {
            expected_request,
            requests: Mutex::new(Vec::new()),
            has_access,
        })
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl Provider for PrepareOnlyCleanupDecryptProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "prepare-only cleanup decrypt provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["decrypt"]
    }

    fn name(&self) -> &'static str {
        "prepare-only-cleanup-decrypt-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        let mut inner_request = request.clone();
        inner_request
            .as_object_mut()
            .ok_or_else(|| ProviderError::Provider("invalid decrypt request".to_string()))?
            .remove("_runtime_invocation");
        let validated = ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &serde_json::to_vec(&inner_request)
                .map_err(|_| ProviderError::Provider("invalid decrypt request".to_string()))?,
            derived_device_runtime_issuer(0x21),
            crate::auth::now_ts(),
        )
        .map_err(|_| ProviderError::Provider("invalid decrypt request".to_string()))?;
        match request.get("op").and_then(Value::as_str) {
            Some("prepare_recipient") => Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_prepared_recipient(
                        validated.audit_request_id(),
                        opaque_handle(0x71),
                        recipient_public_key(0x30),
                        &recipient_identity(0x30),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            Some("close_viewer_session") => Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_viewer_session_already_absent(
                        validated.audit_request_id(),
                        *validated.viewer_session_handle().map_err(|_| {
                            ProviderError::Provider("missing viewer handle".to_string())
                        })?,
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            Some("cancel_prepared_recipient") => Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                        validated.audit_request_id(),
                        *validated.prepared_recipient_handle().map_err(|_| {
                            ProviderError::Provider("missing prepared handle".to_string())
                        })?,
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            _ => Err(ProviderError::Provider(
                "unsupported decrypt operation".to_string(),
            )),
        }
    }
}

fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
}

fn encrypted_content(seed: u8) -> EncryptedContentIdentityV1 {
    EncryptedContentIdentityV1::new(digest(seed), 4096).unwrap()
}

fn content_access_id(seed: u8) -> ContentAccessIdV1 {
    ContentAccessIdV1::new([seed; 16]).unwrap()
}

fn wallet(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

fn runtime_operation_issuer_for_seed(seed: u8) -> RuntimeOperationIssuerKeyV1 {
    RuntimeOperationIssuerKeyV1::new(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
    .unwrap()
}

fn sign_runtime_statement_for_seed_0x21(message: &[u8]) -> [u8; 64] {
    SigningKey::from_bytes(&[0x21; 32]).sign(message).to_bytes()
}

#[cfg(unix)]
fn required_test_binary_path(env_name: &str) -> PathBuf {
    let path = PathBuf::from(
        env::var_os(env_name).unwrap_or_else(|| panic!("missing test binary env: {env_name}")),
    );
    let metadata = fs::metadata(&path)
        .unwrap_or_else(|error| panic!("invalid test binary path {}: {error}", path.display()));
    assert!(
        metadata.is_file(),
        "test binary is not a file: {}",
        path.display()
    );
    assert!(
        metadata.permissions().mode() & 0o111 != 0,
        "test binary is not executable: {}",
        path.display()
    );
    path
}

#[cfg(unix)]
fn runtime_issuer_hex(seed: u8) -> String {
    format!(
        "0x{}",
        hex::encode(runtime_operation_issuer_for_seed(seed).as_bytes())
    )
}

fn wallet_address_hex(wallet: WalletAddress) -> String {
    format!("0x{}", hex::encode(wallet.as_bytes()))
}

#[cfg(unix)]
fn collect_file_bytes(root: &Path, out: &mut Vec<Vec<u8>>) {
    let mut entries = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_file_bytes(&path, out);
        } else if path.is_file() {
            out.push(fs::read(path).unwrap());
        }
    }
}

#[cfg(unix)]
fn any_file_contains(root: &Path, needle: &[u8]) -> bool {
    if !root.exists() {
        return false;
    }
    let mut files = Vec::new();
    collect_file_bytes(root, &mut files);
    files.into_iter().any(|bytes| {
        !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
    })
}

#[cfg(unix)]
async fn invoke_typed_protect_provider(
    registry: &ProviderRegistry,
    op: &str,
    request: &ProtectProviderRequestV1,
) -> ProtectProviderResponseV1 {
    let data = invoke_json_provider(
        registry,
        "protect",
        op,
        serde_json::to_value(request).unwrap(),
    )
    .await
    .unwrap();
    ProtectProviderResponseV1::from_json_slice(&serde_json::to_vec(&data).unwrap()).unwrap()
}

#[cfg(unix)]
fn provision_custody_node_public_receipt(
    binary: &Path,
    state_root: &Path,
    runtime_issuer_seed: u8,
) -> ProvisionedCustodyProviderPublicKeys {
    let output = Command::new(binary)
        .args([
            "provision",
            "--base-path",
            state_root.to_str().unwrap(),
            "--trusted-runtime-issuer",
            &runtime_issuer_hex(runtime_issuer_seed),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    parse_and_verify_provisioning_output(
        &response,
        runtime_operation_issuer_for_seed(runtime_issuer_seed),
    )
    .unwrap()
}

fn node_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
}

fn xwing_public_key_bytes(
    seed: u8,
) -> [u8; elastos_protected_content_contracts::PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
    let secret = x_wing::DecapsulationKey::from([seed; x_wing::DECAPSULATION_KEY_SIZE]);
    secret.encapsulation_key().to_bytes().into()
}

fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
    RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(seed.max(9))).unwrap()
}

fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
    recipient_public_key(seed)
        .key_identity(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1)
        .unwrap()
}

fn node_custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
    NodeCustodyPublicKeyV1::new(xwing_public_key_bytes(seed)).unwrap()
}

fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let size = (8 + content.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(content);
    out
}

fn make_fullbox(box_type: &[u8; 4], flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut content = vec![0u8];
    content.extend_from_slice(&flags.to_be_bytes()[1..]);
    content.extend_from_slice(payload);
    make_box(box_type, &content)
}

fn make_sinf(original_fourcc: &[u8; 4]) -> Vec<u8> {
    let frma = make_box(b"frma", original_fourcc);
    let mut schm_payload = Vec::new();
    schm_payload.extend_from_slice(b"cenc");
    schm_payload.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let schm = make_fullbox(b"schm", 0, &schm_payload);
    let mut tenc_payload = vec![0, 0, 1, 8];
    tenc_payload.extend_from_slice(&[0x44; 16]);
    let tenc = make_fullbox(b"tenc", 0, &tenc_payload);
    let schi = make_box(b"schi", &tenc);
    let mut sinf_content = Vec::new();
    sinf_content.extend_from_slice(&frma);
    sinf_content.extend_from_slice(&schm);
    sinf_content.extend_from_slice(&schi);
    make_box(b"sinf", &sinf_content)
}

fn make_track(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
    let mut tkhd_payload = vec![0u8; 12];
    tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
    let tkhd = make_fullbox(b"tkhd", 0, &tkhd_payload);
    let mut hdlr_payload = vec![0u8; 4];
    hdlr_payload.extend_from_slice(handler_type);
    let hdlr = make_fullbox(b"hdlr", 0, &hdlr_payload);
    let (entry_type, orig_type, fixed) = match handler_type {
        b"vide" => (b"encv", b"avc1", 78usize),
        b"soun" => (b"enca", b"mp4a", 28usize),
        _ => panic!("unsupported handler"),
    };
    let mut entry_content = vec![0u8; fixed];
    entry_content.extend_from_slice(&make_sinf(orig_type));
    let entry = make_box(entry_type, &entry_content);
    let mut stsd_payload = vec![0u8; 4];
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&entry);
    let stsd = make_box(b"stsd", &stsd_payload);
    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mut mdia_content = Vec::new();
    mdia_content.extend_from_slice(&hdlr);
    mdia_content.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_content);
    let mut trak_content = Vec::new();
    trak_content.extend_from_slice(&tkhd);
    trak_content.extend_from_slice(&mdia);
    make_box(b"trak", &trak_content)
}

fn make_clear_track(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
    let mut tkhd_payload = vec![0u8; 12];
    tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
    let tkhd = make_fullbox(b"tkhd", 0, &tkhd_payload);
    let mut hdlr_payload = vec![0u8; 4];
    hdlr_payload.extend_from_slice(handler_type);
    let hdlr = make_fullbox(b"hdlr", 0, &hdlr_payload);
    let (entry_type, fixed) = match handler_type {
        b"vide" => (b"avc1", 78usize),
        b"soun" => (b"mp4a", 28usize),
        _ => panic!("unsupported handler"),
    };
    let entry = make_box(entry_type, &vec![0u8; fixed]);
    let mut stsd_payload = vec![0u8; 4];
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&entry);
    let stsd = make_box(b"stsd", &stsd_payload);
    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mut mdia_content = Vec::new();
    mdia_content.extend_from_slice(&hdlr);
    mdia_content.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_content);
    let mut trak_content = Vec::new();
    trak_content.extend_from_slice(&tkhd);
    trak_content.extend_from_slice(&mdia);
    make_box(b"trak", &trak_content)
}

fn make_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut tfhd_payload = Vec::new();
    tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
    tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
    tfhd_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
    let tfhd = make_fullbox(b"tfhd", 0x020038, &tfhd_payload);
    let tfdt = make_fullbox(b"tfdt", 0, &1u32.to_be_bytes());
    let mut trun_payload = Vec::new();
    trun_payload.extend_from_slice(&1u32.to_be_bytes());
    trun_payload.extend_from_slice(&0i32.to_be_bytes());
    trun_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let trun = make_fullbox(b"trun", 0x000201, &trun_payload);
    let mut senc_payload = Vec::new();
    senc_payload.extend_from_slice(&1u32.to_be_bytes());
    senc_payload.extend_from_slice(&(0x10u64 + u64::from(track_id)).to_be_bytes());
    let senc = make_fullbox(b"senc", 0, &senc_payload);
    let mut traf_content = Vec::new();
    traf_content.extend_from_slice(&tfhd);
    traf_content.extend_from_slice(&tfdt);
    traf_content.extend_from_slice(&trun);
    traf_content.extend_from_slice(&senc);
    let traf = make_box(b"traf", &traf_content);
    let mfhd = make_fullbox(b"mfhd", 0, &1u32.to_be_bytes());
    let mut moof_content = Vec::new();
    moof_content.extend_from_slice(&mfhd);
    moof_content.extend_from_slice(&traf);
    let mut moof = make_box(b"moof", &moof_content);
    let data_offset = (moof.len() + 8) as i32;
    let trun_offset = moof
        .windows(4)
        .position(|window| window == b"trun")
        .expect("trun box present")
        - 4;
    let trun_data_offset_at = trun_offset + 16;
    moof[trun_data_offset_at..trun_data_offset_at + 4].copy_from_slice(&data_offset.to_be_bytes());
    let mdat = make_box(b"mdat", payload);
    let mut out = moof;
    out.extend_from_slice(&mdat);
    out
}

fn make_clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut tfhd_payload = Vec::new();
    tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
    tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
    tfhd_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
    let tfhd = make_fullbox(b"tfhd", 0x020038, &tfhd_payload);
    let tfdt = make_fullbox(b"tfdt", 0, &1u32.to_be_bytes());
    let mut trun_payload = Vec::new();
    trun_payload.extend_from_slice(&1u32.to_be_bytes());
    trun_payload.extend_from_slice(&0i32.to_be_bytes());
    trun_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let trun = make_fullbox(b"trun", 0x000201, &trun_payload);
    let mut traf_content = Vec::new();
    traf_content.extend_from_slice(&tfhd);
    traf_content.extend_from_slice(&tfdt);
    traf_content.extend_from_slice(&trun);
    let traf = make_box(b"traf", &traf_content);
    let mfhd = make_fullbox(b"mfhd", 0, &1u32.to_be_bytes());
    let mut moof_content = Vec::new();
    moof_content.extend_from_slice(&mfhd);
    moof_content.extend_from_slice(&traf);
    let mut moof = make_box(b"moof", &moof_content);
    let data_offset = (moof.len() + 8) as i32;
    let trun_offset = moof
        .windows(4)
        .position(|window| window == b"trun")
        .expect("trun box present")
        - 4;
    let trun_data_offset_at = trun_offset + 16;
    moof[trun_data_offset_at..trun_data_offset_at + 4].copy_from_slice(&data_offset.to_be_bytes());
    let mdat = make_box(b"mdat", payload);
    let mut out = moof;
    out.extend_from_slice(&mdat);
    out
}

pub(crate) fn media_components(seed: u8) -> (Vec<u8>, Vec<Vec<u8>>) {
    let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
    let trak_video = make_track(1, b"vide");
    let trak_audio = make_track(2, b"soun");
    let trex_video = make_fullbox(
        b"trex",
        0,
        &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let trex_audio = make_fullbox(
        b"trex",
        0,
        &[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let mut mvex_content = Vec::new();
    mvex_content.extend_from_slice(&trex_video);
    mvex_content.extend_from_slice(&trex_audio);
    let mvex = make_box(b"mvex", &mvex_content);
    let mvhd = make_box(b"mvhd", &[0u8; 4]);
    let mut moov_content = Vec::new();
    moov_content.extend_from_slice(&mvhd);
    moov_content.extend_from_slice(&trak_video);
    moov_content.extend_from_slice(&trak_audio);
    moov_content.extend_from_slice(&mvex);
    let moov = make_box(b"moov", &moov_content);
    let encrypted_segments = [0usize, 1]
        .into_iter()
        .map(|index| {
            let track_id = if index % 2 == 0 { 1 } else { 2 };
            let payload = vec![
                seed,
                track_id as u8,
                (index & 0xff) as u8,
                ((index >> 8) & 0xff) as u8,
                b's',
                b'e',
                b'g',
                b'x',
            ];
            make_segment(track_id, &payload)
        })
        .collect();
    ([ftyp, moov].concat(), encrypted_segments)
}

fn clear_media_components(seed: u8) -> (Vec<u8>, Vec<Vec<u8>>) {
    let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
    let trak_video = make_clear_track(1, b"vide");
    let trak_audio = make_clear_track(2, b"soun");
    let trex_video = make_fullbox(
        b"trex",
        0,
        &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let trex_audio = make_fullbox(
        b"trex",
        0,
        &[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let mut mvex_content = Vec::new();
    mvex_content.extend_from_slice(&trex_video);
    mvex_content.extend_from_slice(&trex_audio);
    let mvex = make_box(b"mvex", &mvex_content);
    let mvhd = make_box(b"mvhd", &[0u8; 4]);
    let mut moov_content = Vec::new();
    moov_content.extend_from_slice(&mvhd);
    moov_content.extend_from_slice(&trak_video);
    moov_content.extend_from_slice(&trak_audio);
    moov_content.extend_from_slice(&mvex);
    let moov = make_box(b"moov", &moov_content);
    let clear_segments = [0usize, 1]
        .into_iter()
        .map(|index| {
            let track_id = if index % 2 == 0 { 1 } else { 2 };
            let payload = vec![
                seed,
                track_id as u8,
                (index & 0xff) as u8,
                ((index >> 8) & 0xff) as u8,
                b's',
                b'e',
                b'g',
                b'x',
            ];
            make_clear_segment(track_id, &payload)
        })
        .collect();
    ([ftyp, moov].concat(), clear_segments)
}

fn media_identity(seed: u8) -> CencFmp4MediaIdentityV1 {
    let (init_segment, encrypted_segments) = media_components(seed);
    CencFmp4MediaIdentityV1::new_from_bytes(
        &init_segment,
        &encrypted_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
    )
    .unwrap()
}

fn protected_content_directory_from_parts(
    init: &[u8],
    segments: &[Vec<u8>],
    media: &CencFmp4MediaIdentityV1,
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let protected_root = directory.path().join("protected-content/v1/segments");
    fs::create_dir_all(&protected_root).unwrap();
    fs::write(
        directory.path().join("protected-content/v1/identity.bin"),
        media.canonical_bytes().unwrap(),
    )
    .unwrap();
    fs::write(directory.path().join("protected-content/v1/init.mp4"), init).unwrap();
    for (index, segment) in segments.iter().enumerate() {
        fs::write(protected_root.join(format!("{index:08}.m4s")), segment).unwrap();
    }
    directory
}

fn availability_requirement(
    expected_provider_did: impl Into<String>,
) -> RuntimeContentAvailabilityRequirement {
    RuntimeContentAvailabilityRequirement::new(
        expected_provider_did,
        "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#content",
        "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#publisher",
        "protected-content-replication/v1",
        3,
        60,
        5,
    )
    .unwrap()
}

fn protected_content_directory(seed: u8) -> (tempfile::TempDir, CencFmp4MediaIdentityV1) {
    let (init, segments) = media_components(seed);
    let media = CencFmp4MediaIdentityV1::new_from_bytes(
        &init,
        &segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
    )
    .unwrap();
    (
        protected_content_directory_from_parts(&init, &segments, &media),
        media,
    )
}

async fn publish_protected_content_for_test(
    config: ContentAvailabilityTestConfig,
    expected_provider_did: Option<String>,
    mutate_directory: Option<fn(&Path)>,
) -> anyhow::Result<elastos_protected_content_runtime::RuntimeVerifiedContentAvailability> {
    let (directory, media) = protected_content_directory(0x41);
    if let Some(mutate_directory) = mutate_directory {
        mutate_directory(directory.path());
    }
    let provider = ContentAvailabilityTestProvider::new(0x61, config);
    let expected_provider_did = expected_provider_did.unwrap_or_else(|| provider.signer_did());
    let requirement = availability_requirement(expected_provider_did);
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("content", provider)
        .await
        .unwrap();
    super::publish_and_verify_protected_content_availability(
        registry.as_ref(),
        directory.path(),
        &media,
        &requirement,
        NOW,
    )
    .await
}

struct RuntimeCustodyPrebuyAvailabilityHarness {
    _temp: tempfile::TempDir,
    data_dir: PathBuf,
    registry: Arc<ProviderRegistry>,
    content_provider: Arc<ContentAvailabilityTestProvider>,
    mint_id: Digest32,
}

fn runtime_custody_listing_record_for_test(
    mint_id: Digest32,
    publisher_principal_id: &str,
    display_name: &str,
    published_at: u64,
) -> super::RuntimeCustodyListingRecord {
    let draft = mint_draft_for_composition_journal_test();
    let publisher_profile_did = derived_device_key_for_seed(0x66).1;
    let content_id = super::runtime_protected_content_id(draft.encrypted_content()).unwrap();
    let package = super::RuntimePortableListingPackage {
        schema: super::RUNTIME_PORTABLE_LISTING_SCHEMA_V1.to_string(),
        mint_id: hex::encode(mint_id.as_bytes()),
        content_id,
        content_access_id: format!("0x{}", hex::encode(draft.content_access_id().as_bytes())),
        content_cid: ContentAvailabilityTestProvider::CID.to_string(),
        metadata_cid: ContentAvailabilityTestProvider::CID.to_string(),
        token_uri: format!(
            "ipfs://{}/metadata.json",
            ContentAvailabilityTestProvider::CID
        ),
        publisher_profile_did,
        display_name: display_name.to_string(),
        media_identity_base64: base64::engine::general_purpose::STANDARD
            .encode(draft.media_identity().canonical_bytes().unwrap()),
        key_envelope_identity_base64: base64::engine::general_purpose::STANDARD
            .encode(draft.key_envelope().canonical_bytes().unwrap()),
        rights_policy_identity_base64: base64::engine::general_purpose::STANDARD
            .encode(draft.policy().canonical_bytes().unwrap()),
        content_key_commitment_base64: base64::engine::general_purpose::STANDARD
            .encode(draft.content_key_commitment().as_bytes()),
        quantity: "0x2".to_string(),
        seller_address: "0x0000000000000000000000000000000000000011".to_string(),
        chain_namespace: "eip155:8453".to_string(),
        network: "base-mainnet".to_string(),
        ledger: "0x0000000000000000000000000000000000000022".to_string(),
        token_id: "0x1".to_string(),
        operative: "0x0000000000000000000000000000000000000033".to_string(),
        price: "0x5".to_string(),
        pay_token: "0x0000000000000000000000000000000000000044".to_string(),
        payment_processor: Some("0x0000000000000000000000000000000000000055".to_string()),
        mint_transaction_hash: format!("0x{}", hex::encode([0x68; 32])),
        published_at,
    };
    super::RuntimeCustodyListingRecord {
        schema: super::RUNTIME_LISTING_SCHEMA_V1.to_string(),
        origin: super::RuntimeCustodyListingOrigin::LocalCreator {
            principal_id: publisher_principal_id.to_string(),
            listing_uri: format!("elastos://{}", ContentAvailabilityTestProvider::CID),
            package_sha256: hex::encode(sha2::Sha256::digest(
                serde_json::to_vec(&package).unwrap(),
            )),
        },
        package,
        availability: super::RuntimeCustodyListingAvailabilitySummary {
            checked_at: NOW,
            observed_replicas: 3,
            receipt_digest: hex::encode([0x67; 32]),
        },
    }
}

async fn runtime_custody_prebuy_availability_harness(
    provider_seed: u8,
    provider_config: ContentAvailabilityTestConfig,
) -> RuntimeCustodyPrebuyAvailabilityHarness {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&data_dir.join("protected-content"));
    write_device_key(&data_dir, 0x61);
    write_library_publish_test_composition(&data_dir);

    let mint_draft = mint_draft_for_composition_journal_test();
    let fixture_now = crate::auth::now_ts();
    let mint_journal = runtime_mint_journal(&data_dir);
    mint_journal.persist_bound(&mint_draft).unwrap();
    for (index, node) in mint_draft.nodes().iter().enumerate() {
        let seed = u8::try_from(index + 1).unwrap();
        mint_journal
            .mark_node_effect_started(mint_draft.mint_id(), node.node_public_key())
            .unwrap();
        mint_journal
            .mark_node_receipt(
                mint_draft.mint_id(),
                RuntimeMintNodeReceipt::new(
                    node.node_public_key(),
                    RuntimeCustodyProvisioningIdV1::new(digest(0x80 + seed)).unwrap(),
                    CustodyNodeProvisioningRecordIdentityV1::new(digest(0xa0 + seed), 128).unwrap(),
                    node.owner_state_root(),
                )
                .unwrap(),
            )
            .unwrap();
    }
    let custody_provisioned = mint_journal
        .mark_custody_provisioned(mint_draft.mint_id())
        .unwrap();
    assert_eq!(
        custody_provisioned.custody_terminal(),
        Some(elastos_protected_content_runtime::RuntimeCustodyTerminalKind::CustodyProvisioned)
    );

    let expected_provider_did =
        ContentAvailabilityTestProvider::new(0x61, ContentAvailabilityTestConfig::accepted())
            .signer_did();
    let requirement = RuntimeContentAvailabilityRequirement::new(
        expected_provider_did,
        super::runtime_protected_content_id(mint_draft.encrypted_content()).unwrap(),
        derived_device_key_for_seed(0x66).1,
        super::PROTECTED_CONTENT_REPLICATION_POLICY,
        super::PROTECTED_CONTENT_MIN_REPLICAS,
        super::PROTECTED_CONTENT_AVAILABILITY_MAX_AGE_SECS,
        super::PROTECTED_CONTENT_AVAILABILITY_MAX_FUTURE_SKEW_SECS,
    )
    .unwrap();
    let evidence = elastos_protected_content_runtime::RuntimeVerifiedContentAvailability::new(
        ContentAvailabilityTestProvider::CID,
        requirement.expected_object_identity(),
        requirement.expected_publisher_did(),
        &requirement,
        3,
        fixture_now.saturating_sub(1),
        digest(0x71),
        mint_draft.encrypted_content().clone(),
        mint_draft.media_identity().media_manifest_root(),
    )
    .unwrap();
    let available = mint_journal
        .mark_content_available(mint_draft.mint_id(), &requirement, evidence)
        .unwrap();
    assert_eq!(available.draft().mint_id(), mint_draft.mint_id());
    assert!(available.content_availability().is_some());

    let listing = runtime_custody_listing_record_for_test(
        mint_draft.mint_id(),
        "person:local:creator",
        "protected-video.mp4",
        fixture_now.saturating_sub(2),
    );
    super::persist_runtime_custody_listing(&data_dir, &listing).unwrap();

    let (init_segment, encrypted_segments) = media_components(0x41);
    let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
        &init_segment,
        &encrypted_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
    )
    .unwrap();
    let protected_directory =
        protected_content_directory_from_parts(&init_segment, &encrypted_segments, &media_identity);
    let content_provider = ContentAvailabilityTestProvider::new(provider_seed, provider_config);
    content_provider
        .seed_published_protected_content(
            protected_directory.path(),
            requirement.expected_object_identity(),
            requirement.expected_publisher_did(),
        )
        .await
        .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("content", content_provider.clone())
        .await
        .unwrap();

    RuntimeCustodyPrebuyAvailabilityHarness {
        _temp: temp,
        data_dir,
        registry,
        content_provider,
        mint_id: mint_draft.mint_id(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_listings_project_public_summary_and_access_state() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let buyer_principal_id = "person:local:buyer";
    install_profile_authority_keeping_device_key(&harness.data_dir, buyer_principal_id);
    let buyer_profile_did = load_profile_did_for_test(&harness.data_dir, buyer_principal_id);
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        buyer_principal_id,
        &buyer_profile_did,
        crate::auth::now_ts(),
    );

    let creator_view =
        super::list_runtime_custody_listings(&harness.data_dir, "person:local:creator").unwrap();
    assert_eq!(
        creator_view["schema"],
        super::RUNTIME_CUSTODY_LISTINGS_RESPONSE_SCHEMA_V1
    );
    assert_eq!(creator_view["truncated"], false);
    let creator_listing = creator_view["listings"][0].as_object().unwrap();
    assert_eq!(
        creator_listing.keys().cloned().collect::<Vec<_>>(),
        vec![
            "access_state",
            "availability",
            "codecs",
            "display_name",
            "mime_type",
            "mint_id",
            "pay_token",
            "price",
            "published_at",
            "quantity",
            "schema",
            "seller_address",
            "token_id",
        ]
    );
    assert_eq!(creator_listing["display_name"], "protected-video.mp4");
    assert_eq!(creator_listing["mime_type"], MEDIA_MIME_TYPE_V1);
    assert_eq!(creator_listing["codecs"], MEDIA_CODECS_V1);
    assert_eq!(creator_listing["quantity"], "0x2");
    assert_eq!(creator_listing["price"], "0x5");
    let expected_availability = json!({
        "schema": super::RUNTIME_CUSTODY_LISTING_AVAILABILITY_SCHEMA_V1,
        "status": "last_verified_receipt",
        "checked_at": NOW,
        "required_replicas": 3,
        "observed_replicas": 3,
        "receipt_digest": hex::encode([0x67; 32]),
        "recheck_before_buy": true,
        "recheck_before_open": true,
    });
    assert_eq!(creator_listing["availability"], expected_availability);
    assert_eq!(creator_listing["access_state"], "creator");

    let buyer_view =
        super::list_runtime_custody_listings(&harness.data_dir, buyer_principal_id).unwrap();
    assert_eq!(buyer_view["listings"][0]["access_state"], "purchased");
    assert_eq!(
        buyer_view["listings"][0]["availability"],
        expected_availability
    );

    let other_view =
        super::list_runtime_custody_listings(&harness.data_dir, "person:local:other").unwrap();
    assert_eq!(other_view["listings"][0]["access_state"], "available");
    assert_eq!(
        other_view["listings"][0]["availability"],
        expected_availability
    );
    for hidden in [
        "publisher_principal_id",
        "content_id",
        "cid",
        "metadata_cid",
        "token_uri",
        "chain_namespace",
        "network",
        "ledger",
        "operative",
        "payment_processor",
    ] {
        assert!(
            creator_view["listings"][0].get(hidden).is_none(),
            "{hidden}"
        );
    }
}

#[cfg(unix)]
#[test]
fn runtime_custody_listing_selection_is_stable_and_truncated_by_mint_filename() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&data_dir.join("protected-content"));
    let listings_root = data_dir.join(super::RUNTIME_LISTING_ROOT);
    owner_only_dir(&listings_root);

    for index in (0..(super::MAX_RUNTIME_CUSTODY_LISTINGS + 3)).rev() {
        let mint_id = Digest32::new([u8::try_from(index + 1).unwrap(); 32]);
        let path = super::runtime_listing_path(&data_dir, mint_id);
        write_owner_only_bytes(&path, b"{}").unwrap();
    }

    let (first_paths, first_truncated) =
        super::select_runtime_custody_listing_paths(&listings_root).unwrap();
    let (second_paths, second_truncated) =
        super::select_runtime_custody_listing_paths(&listings_root).unwrap();
    assert_eq!(first_truncated, true);
    assert_eq!(second_truncated, true);
    assert_eq!(first_paths, second_paths);
    assert_eq!(first_paths.len(), super::MAX_RUNTIME_CUSTODY_LISTINGS);
    let selected = first_paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let expected = (0..super::MAX_RUNTIME_CUSTODY_LISTINGS)
        .map(|index| {
            format!(
                "{}.json",
                hex::encode([u8::try_from(index + 1).unwrap(); 32])
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(selected, expected);
}

#[cfg(unix)]
#[test]
fn runtime_custody_listings_reject_malformed_record() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&data_dir.join("protected-content"));

    let mint_id = digest(0x90);
    let mut invalid = serde_json::to_value(runtime_custody_listing_record_for_test(
        mint_id,
        "person:local:creator",
        "Valid name",
        123,
    ))
    .unwrap();
    invalid["display_name"] = Value::String(String::new());
    write_owner_only_bytes(
        &super::runtime_listing_path(&data_dir, mint_id),
        &serde_json::to_vec(&invalid).unwrap(),
    )
    .unwrap();

    let error = super::list_runtime_custody_listings(&data_dir, "person:local:creator")
        .err()
        .expect("expected malformed record rejection");
    assert_eq!(error.to_string(), "Runtime custody listing is invalid");
}

#[tokio::test]
async fn protected_content_availability_publishes_status_refetches_and_verifies_exact_media() {
    let evidence =
        publish_protected_content_for_test(ContentAvailabilityTestConfig::accepted(), None, None)
            .await
            .unwrap();
    assert_eq!(
        evidence.object_identity(),
        "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#content"
    );
    assert_eq!(
        evidence.publisher_identity(),
        "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#publisher"
    );
    assert_eq!(evidence.required_replicas(), 3);
    assert_eq!(evidence.observed_replicas(), 3);
    let debug = format!("{evidence:?}");
    assert!(!debug.contains("init.mp4"));
    assert!(!debug.contains("segments/"));
}

#[tokio::test]
async fn protected_content_publish_sends_exact_three_replica_live_requirement() {
    let (directory, media) = protected_content_directory(0x41);
    let provider =
        ContentAvailabilityTestProvider::new(0x61, ContentAvailabilityTestConfig::accepted());
    let requirement = availability_requirement(provider.signer_did());
    let registry = ProviderRegistry::new();
    registry
        .register_sub_provider("content", provider.clone())
        .await
        .unwrap();

    super::publish_and_verify_protected_content_availability(
        &registry,
        directory.path(),
        &media,
        &requirement,
        NOW,
    )
    .await
    .unwrap();

    let requests = provider.requests().await;
    let publish = requests
        .iter()
        .find(|request| request.get("op").and_then(Value::as_str) == Some("publish"))
        .unwrap();
    assert_eq!(
        publish.get("availability_requirements"),
        Some(&json!({
            "min_replicas": 3,
            "require_live_multi_peer_proof": true,
        }))
    );
    assert!(publish["availability_requirements"]
        .get("max_replicas")
        .is_none());
}

#[tokio::test]
async fn protected_content_publish_rejects_mismatched_policy_or_count_before_provider_use() {
    for (policy, minimum_replicas) in [("wrong-policy", 3), ("protected-content-replication/v1", 2)]
    {
        let (directory, media) = protected_content_directory(0x41);
        let provider =
            ContentAvailabilityTestProvider::new(0x61, ContentAvailabilityTestConfig::accepted());
        let requirement = RuntimeContentAvailabilityRequirement::new(
            provider.signer_did(),
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#content",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#publisher",
            policy,
            minimum_replicas,
            60,
            5,
        )
        .unwrap();
        let registry = ProviderRegistry::new();
        registry
            .register_sub_provider("content", provider.clone())
            .await
            .unwrap();

        super::publish_and_verify_protected_content_availability(
            &registry,
            directory.path(),
            &media,
            &requirement,
            NOW,
        )
        .await
        .expect_err("mismatched protected availability requirement must fail closed");

        assert!(provider.requests().await.is_empty());
    }
}

#[tokio::test]
async fn protected_content_availability_rejects_wrong_signed_receipt_or_refetched_object() {
    let wrong_signer =
        ContentAvailabilityTestProvider::new(0x62, ContentAvailabilityTestConfig::accepted())
            .signer_did();
    assert!(publish_protected_content_for_test(
        ContentAvailabilityTestConfig::accepted(),
        Some(wrong_signer),
        None,
    )
    .await
    .is_err());
    for config in [
        ContentAvailabilityTestConfig {
            receipt_object_identity: Some("did:key:wrong#content".to_string()),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            receipt_publisher_did: Some("did:key:wrong#publisher".to_string()),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            policy: "wrong-policy".to_string(),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            status: "local_pinned".to_string(),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            replicas: 2,
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            live_multi_peer_proof: None,
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            live_multi_peer_proof: Some(false),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            malformed_receipt: true,
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            checked_at: NOW - 61,
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            checked_at: NOW + 6,
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            receipt_cid: Some(
                "bafybeibwzif2r5tn7z7cq4f5a2mmepmab4s4m5a2hqu5v4f4uzkd3t2u7m".to_string(),
            ),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            mutate_fetch_path: Some("protected-content/v1/identity.bin".to_string()),
            ..ContentAvailabilityTestConfig::accepted()
        },
        ContentAvailabilityTestConfig {
            mutate_manifest_extra_file: true,
            ..ContentAvailabilityTestConfig::accepted()
        },
    ] {
        assert!(publish_protected_content_for_test(config, None, None)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn runtime_custody_prebuy_availability_refetches_fresh_exact_receipt_without_publish() {
    let now = crate::auth::now_ts();
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig {
            checked_at: now,
            ..ContentAvailabilityTestConfig::accepted()
        },
    )
    .await;
    let (_, verified) = super::verify_fresh_runtime_custody_availability(
        &harness.data_dir,
        &harness.registry,
        harness.mint_id,
        now,
    )
    .await
    .unwrap();
    assert_eq!(verified.content_cid(), ContentAvailabilityTestProvider::CID);
    assert_eq!(verified.required_replicas(), 3);
    assert_eq!(verified.observed_replicas(), 3);

    let requests = harness.content_provider.requests().await;
    assert!(requests
        .iter()
        .any(|request| request.get("op").and_then(Value::as_str) == Some("status")));
    assert!(requests
        .iter()
        .any(|request| request.get("op").and_then(Value::as_str) == Some("fetch")));
    assert!(requests.iter().all(|request| matches!(
        request.get("op").and_then(Value::as_str),
        Some("status" | "fetch")
    )));
    assert!(!requests
        .iter()
        .any(|request| request.get("op").and_then(Value::as_str) == Some("publish")));
}

#[tokio::test]
async fn runtime_custody_prebuy_availability_rejects_stale_receipt() {
    let now = crate::auth::now_ts();
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig {
            checked_at: now - super::PROTECTED_CONTENT_AVAILABILITY_MAX_AGE_SECS - 1,
            ..ContentAvailabilityTestConfig::accepted()
        },
    )
    .await;
    assert!(super::verify_fresh_runtime_custody_availability(
        &harness.data_dir,
        &harness.registry,
        harness.mint_id,
        now,
    )
    .await
    .is_err());
}

#[tokio::test]
async fn runtime_custody_prebuy_availability_rejects_wrong_receipt_binding_or_manifest() {
    let now = crate::auth::now_ts();
    let cases = [
        ContentAvailabilityTestConfig {
            receipt_object_identity: Some("did:key:wrong#content".to_string()),
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            receipt_publisher_did: Some("did:key:wrong#publisher".to_string()),
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            policy: "wrong-policy".to_string(),
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            replicas: 2,
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            live_multi_peer_proof: None,
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            live_multi_peer_proof: Some(false),
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            malformed_receipt: true,
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
        ContentAvailabilityTestConfig {
            mutate_manifest_extra_file: true,
            ..ContentAvailabilityTestConfig::accepted_at(now)
        },
    ];
    for config in cases {
        let harness = runtime_custody_prebuy_availability_harness(0x61, config).await;
        assert!(super::verify_fresh_runtime_custody_availability(
            &harness.data_dir,
            &harness.registry,
            harness.mint_id,
            now,
        )
        .await
        .is_err());
    }

    let wrong_signer = runtime_custody_prebuy_availability_harness(
        0x62,
        ContentAvailabilityTestConfig::accepted_at(now),
    )
    .await;
    assert!(super::verify_fresh_runtime_custody_availability(
        &wrong_signer.data_dir,
        &wrong_signer.registry,
        wrong_signer.mint_id,
        now,
    )
    .await
    .is_err());
}

#[tokio::test]
async fn runtime_custody_prebuy_availability_requires_existing_mint_and_listing_before_provider_use(
) {
    let missing_listing = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    fs::remove_file(super::runtime_listing_path(
        &missing_listing.data_dir,
        missing_listing.mint_id,
    ))
    .unwrap();
    assert!(super::verify_fresh_runtime_custody_availability(
        &missing_listing.data_dir,
        &missing_listing.registry,
        missing_listing.mint_id,
        NOW,
    )
    .await
    .is_err());
    assert!(missing_listing.content_provider.requests().await.is_empty());

    let wrong_mint = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    assert!(super::verify_fresh_runtime_custody_availability(
        &wrong_mint.data_dir,
        &wrong_mint.registry,
        digest(0x33),
        NOW,
    )
    .await
    .is_err());
    assert!(wrong_mint.content_provider.requests().await.is_empty());
}

fn mutate_protected_content_descriptor(directory: &Path) {
    fs::write(
        directory.join("protected-content/v1/identity.bin"),
        b"not-a-canonical-media-identity",
    )
    .unwrap();
}

fn mutate_protected_content_segment(directory: &Path) {
    let path = directory.join("protected-content/v1/segments/00000000.m4s");
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 0x01;
    fs::write(path, bytes).unwrap();
}

fn remove_protected_content_descriptor(directory: &Path) {
    fs::remove_file(directory.join("protected-content/v1/identity.bin")).unwrap();
}

fn rename_protected_content_descriptor(directory: &Path) {
    fs::rename(
        directory.join("protected-content/v1/identity.bin"),
        directory.join("protected-content/v1/identity-renamed.bin"),
    )
    .unwrap();
}

fn remove_protected_content_init(directory: &Path) {
    fs::remove_file(directory.join("protected-content/v1/init.mp4")).unwrap();
}

fn rename_protected_content_init(directory: &Path) {
    fs::rename(
        directory.join("protected-content/v1/init.mp4"),
        directory.join("protected-content/v1/init-renamed.mp4"),
    )
    .unwrap();
}

fn remove_protected_content_segment(directory: &Path) {
    fs::remove_file(directory.join("protected-content/v1/segments/00000000.m4s")).unwrap();
}

fn rename_protected_content_segment(directory: &Path) {
    fs::rename(
        directory.join("protected-content/v1/segments/00000000.m4s"),
        directory.join("protected-content/v1/segments/00000000-renamed.m4s"),
    )
    .unwrap();
}

fn reorder_protected_content_segments(directory: &Path) {
    let first = directory.join("protected-content/v1/segments/00000000.m4s");
    let second = directory.join("protected-content/v1/segments/00000001.m4s");
    let first_bytes = fs::read(&first).unwrap();
    let second_bytes = fs::read(&second).unwrap();
    fs::write(first, second_bytes).unwrap();
    fs::write(second, first_bytes).unwrap();
}

fn add_protected_content_segment_index_alias(directory: &Path) {
    fs::copy(
        directory.join("protected-content/v1/segments/00000000.m4s"),
        directory.join("protected-content/v1/segments/00000000-copy.m4s"),
    )
    .unwrap();
}

fn truncate_protected_content_descriptor(directory: &Path) {
    let path = directory.join("protected-content/v1/identity.bin");
    let mut bytes = fs::read(&path).unwrap();
    bytes.pop().unwrap();
    fs::write(path, bytes).unwrap();
}

fn mutate_protected_content_init(directory: &Path) {
    let path = directory.join("protected-content/v1/init.mp4");
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 0x01;
    fs::write(path, bytes).unwrap();
}

fn truncate_protected_content_init(directory: &Path) {
    let path = directory.join("protected-content/v1/init.mp4");
    let mut bytes = fs::read(&path).unwrap();
    bytes.pop().unwrap();
    fs::write(path, bytes).unwrap();
}

fn truncate_protected_content_segment(directory: &Path) {
    let path = directory.join("protected-content/v1/segments/00000000.m4s");
    let mut bytes = fs::read(&path).unwrap();
    bytes.pop().unwrap();
    fs::write(path, bytes).unwrap();
}

fn add_extra_protected_content_file(directory: &Path) {
    fs::write(directory.join("protected-content/v1/extra.bin"), b"extra").unwrap();
}

#[tokio::test]
async fn protected_content_availability_rejects_descriptor_layout_and_extra_source_files() {
    for mutate_directory in [
        mutate_protected_content_descriptor as fn(&Path),
        mutate_protected_content_segment,
        add_extra_protected_content_file,
    ] {
        assert!(publish_protected_content_for_test(
            ContentAvailabilityTestConfig::accepted(),
            None,
            Some(mutate_directory),
        )
        .await
        .is_err());
    }
}

#[tokio::test]
async fn protected_content_availability_rejects_fixed_layout_names_indices_sizes_and_digests() {
    for mutate_directory in [
        remove_protected_content_descriptor as fn(&Path),
        rename_protected_content_descriptor,
        remove_protected_content_init,
        rename_protected_content_init,
        remove_protected_content_segment,
        rename_protected_content_segment,
        reorder_protected_content_segments,
        add_protected_content_segment_index_alias,
        truncate_protected_content_descriptor,
        mutate_protected_content_init,
        truncate_protected_content_init,
        mutate_protected_content_segment,
        truncate_protected_content_segment,
    ] {
        assert!(publish_protected_content_for_test(
            ContentAvailabilityTestConfig::accepted(),
            None,
            Some(mutate_directory),
        )
        .await
        .is_err());
    }
}

fn sealed_share(seed: u8) -> PqHybridSealedShareV1 {
    let public = x_wing::EncapsulationKey::new_from_slice(&xwing_public_key_bytes(seed)).unwrap();
    let (ciphertext, _) =
        public.encapsulate_deterministic(&[seed; x_wing::ENCAPSULATION_RANDOMNESS_SIZE].into());
    let ciphertext: [u8; X_WING_DRAFT06_CIPHERTEXT_BYTES] = ciphertext.into();
    let mut envelope = Vec::with_capacity(PQ_HYBRID_SEALED_SHARE_ENVELOPE_BYTES);
    envelope.extend_from_slice(&ciphertext);
    envelope.extend_from_slice(&[seed; PQ_HYBRID_AEAD_NONCE_BYTES]);
    envelope.extend_from_slice(&[seed ^ 0x5a; PQ_HYBRID_WRAPPED_SHARE_BYTES]);
    PqHybridSealedShareV1::new(envelope).unwrap()
}

fn policy_body_for(
    encrypted_content: EncryptedContentIdentityV1,
    content_access_id: ContentAccessIdV1,
    action: RightsActionV1,
) -> RightsPolicyBodyV1 {
    RightsPolicyBodyV1::new(
        encrypted_content,
        content_access_id,
        action,
        elastos_protected_content_contracts::RightsSubjectSourceV1::WalletAddress,
        11155111,
        EvmContractAddressV1::new([0x11; 20]).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
        RightsObservationFinalityV1::finalized(),
    )
    .unwrap()
}

fn policy_body() -> RightsPolicyBodyV1 {
    policy_body_for(
        encrypted_content(0x11),
        content_access_id(0x41),
        RightsActionV1::View,
    )
}

fn signed_custody_epoch() -> SignedCustodyEpochV1 {
    signed_custody_epoch_with_first_node(node_public_key(1), node_custody_public_key(1))
}

fn signed_custody_epoch_with_first_node(
    first_node_public_key: NodePublicKey,
    first_node_custody_public_key: NodeCustodyPublicKeyV1,
) -> SignedCustodyEpochV1 {
    signed_custody_epoch_for_node_keys([
        (first_node_public_key, first_node_custody_public_key),
        (node_public_key(2), node_custody_public_key(2)),
        (node_public_key(3), node_custody_public_key(3)),
    ])
}

fn signed_custody_epoch_for_node_keys(
    nodes: [(NodePublicKey, NodeCustodyPublicKeyV1); 3],
) -> SignedCustodyEpochV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let nodes = nodes
        .into_iter()
        .enumerate()
        .map(|(index, (node_public_key, custody_public_key))| {
            elastos_protected_content_contracts::CustodyNodeIdentityV1::new(
                node_public_key,
                custody_public_key,
                ShareCoordinateV1::new(u8::try_from(index + 1).unwrap()).unwrap(),
            )
            .unwrap()
        })
        .collect();
    let statement = CustodyEpochStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        )
        .unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        nodes,
    )
    .unwrap();
    SignedCustodyEpochV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn signed_custody_pool_for_epoch(
    epoch: &SignedCustodyEpochV1,
    active_window: (u64, u64),
) -> SignedCustodyPoolV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let members = epoch
        .statement()
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let node_seed = u8::try_from(index + 1).unwrap();
            CustodyPoolMemberV1::new(
                node.node_public_key(),
                node.custody_public_key(),
                CustodyPoolOperatorIdV1::new([0x80 + node_seed; 32]),
                CustodyPoolFailureDomainIdV1::new([0x90 + node_seed; 32]),
                CustodyApprovedSuitesV1::new(
                    CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                    CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                    CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                )
                .unwrap(),
                active_window,
                CustodyPoolMemberStateV1::Active,
            )
            .unwrap()
        })
        .collect();
    let statement = CustodyPoolStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        members,
    )
    .unwrap();
    SignedCustodyPoolV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn signed_committee_authorization_for_epoch(
    pool_identity: CustodyPoolIdentityV1,
    epoch: &SignedCustodyEpochV1,
) -> SignedCustodyCommitteeAuthorizationV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let statement = CustodyCommitteeAuthorizationStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        pool_identity,
        epoch.epoch_identity().unwrap(),
    )
    .unwrap();
    SignedCustodyCommitteeAuthorizationV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn validated_custody_committee_for_epoch(
    epoch: &SignedCustodyEpochV1,
    now: u64,
) -> ValidatedCustodyCommitteeV1 {
    let pool = signed_custody_pool_for_epoch(epoch, (now.saturating_sub(60), now + 3600));
    let authorization =
        signed_committee_authorization_for_epoch(pool.pool_identity().unwrap(), epoch);
    elastos_protected_content_contracts::validate_custody_epoch_against_pool_at(
        CustodyEpochIssuerKeyV1::new(
            SigningKey::from_bytes(&[0x71; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        authorization.authorization_identity().unwrap(),
        &pool,
        epoch,
        &authorization,
        now,
    )
    .unwrap()
}

fn custody_envelope_for_media(seed: u8) -> CustodyEnvelopeV1 {
    custody_envelope_for_media_with_epoch(seed, &signed_custody_epoch())
}

fn custody_envelope_for_media_with_epoch(
    seed: u8,
    epoch: &SignedCustodyEpochV1,
) -> CustodyEnvelopeV1 {
    let media = media_identity(seed);
    let manifest = CustodyEnvelopeManifestV1::new(
        media.encrypted_content().clone(),
        CustodyPoolIdentityV1::new(digest(seed ^ 0x34), 512).unwrap(),
        epoch.epoch_identity().unwrap(),
        CustodyCommitteeAuthorizationIdentityV1::new(digest(seed ^ 0x35), 512).unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        digest(seed ^ 0x33),
        epoch.statement().nodes().to_vec(),
    )
    .unwrap();
    let shares = [seed ^ 0x50, seed ^ 0x51, seed ^ 0x52]
        .into_iter()
        .map(sealed_share)
        .collect();
    CustodyEnvelopeV1::new(manifest, shares).unwrap()
}

fn provisioned_custody_envelope_for_media_with_epoch(
    seed: u8,
    epoch: &SignedCustodyEpochV1,
    now: u64,
) -> CustodyEnvelopeV1 {
    let media = media_identity(seed);
    let content_key =
        elastos_protected_content_custody::ContentEncryptionKeyV1::generate().unwrap();
    let committee = validated_custody_committee_for_epoch(epoch, now);
    provision_custody_envelope(media.encrypted_content().clone(), &content_key, &committee).unwrap()
}

fn binding_for_envelope(
    envelope: &CustodyEnvelopeV1,
) -> elastos_protected_content_contracts::ProtectedContentBindingV1 {
    let policy = policy_body();
    elastos_protected_content_contracts::ProtectedContentBindingV1::new(
        envelope.manifest().encrypted_content().clone(),
        envelope.key_envelope_identity().unwrap(),
        policy.policy_identity().unwrap(),
        elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
            SigningKey::from_bytes(&[0x26; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        wallet(7),
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap()
}

#[cfg(unix)]
fn store_profile_signing_passkey(
    data_dir: &Path,
    principal_id: &str,
    credential_id: &str,
) -> String {
    let now = crate::auth::now_ts();
    let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
        elastos_runtime::auth::PasskeyWebAuthnBinding {
            credential_id: credential_id.to_string(),
            public_key: "protected-content-release-test-public-key".to_string(),
            sign_count: 1,
            user_verified: true,
            origin: "https://elastos.elacitylabs.com".to_string(),
            rp_id: "elastos.elacitylabs.com".to_string(),
            created_at: now,
            last_used_at: now,
            revoked_at: None,
        },
    );
    crate::auth::upsert_principal_for_binding_as_role_named(
        data_dir,
        binding,
        principal_id.to_string(),
        crate::auth::RuntimePrincipalRole::Admin,
        None,
        now,
    )
    .unwrap()
    .proof_binding_id
}

#[cfg(unix)]
fn install_profile_authority_keeping_device_key(
    data_dir: &Path,
    principal_id: &str,
) -> (String, ProfileIdentityV1) {
    let proof_binding_id =
        store_profile_signing_passkey(data_dir, principal_id, "credential-protected-content");
    crate::auth::store_test_principal_root_protection(data_dir, principal_id);
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let profile = crate::collaboration_profile_authority::update_profile_authority(
        data_dir,
        principal_id,
        &localhost_root,
        &proof_binding_id,
        "Protected Content Test",
        None,
        crate::auth::now_ts(),
    )
    .unwrap();
    let profile_key = crate::crypto::decode_did_key(&profile.document().profile_did).unwrap();
    let profile_identity =
        ProfileIdentityV1::from_public_key_bytes(profile_key.to_bytes()).unwrap();
    (proof_binding_id, profile_identity)
}

#[cfg(unix)]
fn load_profile_did_for_test(data_dir: &Path, principal_id: &str) -> String {
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    crate::collaboration_profile_authority::load_profile_authority(
        data_dir,
        principal_id,
        &localhost_root,
    )
    .unwrap()
    .unwrap()
    .document()
    .profile_did
    .clone()
}

#[cfg(unix)]
fn persist_runtime_custody_purchase_for_mint(
    data_dir: &Path,
    mint: &PersistedRuntimeMint,
    principal_id: &str,
    profile_did: &str,
    now: u64,
) -> RuntimeCustodyPurchaseRecord {
    let listing = super::load_runtime_custody_listing(data_dir, mint.draft().mint_id())
        .unwrap()
        .expect("runtime custody listing");
    let wallet_binding = ValidatedChainOutcomeBindingV1::ManagedSigned {
        signed_transaction_sha256: format!("sha256:{}", hex::encode([0xab; 32])),
    };
    let chain_transaction = format!("0x{}", hex::encode([0xaa; 32]));
    let chain_observation = json!({
        "schema": "elastos.chain.broadcast_receipt/v1",
        "network": "esc-mainnet",
    });
    let record = RuntimeCustodyPurchaseRecord {
        schema: RUNTIME_PURCHASE_SCHEMA_V1.to_string(),
        principal_id: principal_id.to_string(),
        profile_did: profile_did.to_string(),
        mint_id: hex::encode(mint.draft().mint_id().as_bytes()),
        content_id: runtime_protected_content_id(mint.draft().encrypted_content()).unwrap(),
        cid: mint
            .content_availability()
            .unwrap()
            .content_cid()
            .to_string(),
        listing_sha256: listing.portable_package_digest(),
        seller_address: listing.package.seller_address.clone(),
        chain_namespace: listing.package.chain_namespace.clone(),
        network: listing.package.network.clone(),
        ledger: listing.package.ledger.clone(),
        token_id: listing.package.token_id.clone(),
        operative: listing.package.operative.clone(),
        price: listing.package.price.clone(),
        pay_token: listing.package.pay_token.clone(),
        payment_processor: listing.package.payment_processor.clone(),
        availability_receipt_digest: format!(
            "sha256:{}",
            hex::encode(
                mint.content_availability()
                    .unwrap()
                    .receipt_digest()
                    .as_bytes()
            )
        ),
        account_id: "wallet-account-alpha".to_string(),
        address: wallet_address_hex(wallet(7)),
        approval_stage: None,
        buy_stage: RuntimeCustodyPurchaseStageRecord {
            stage: "buy".to_string(),
            effect_id: "runtime-effect:11111111111111111111111111111111".to_string(),
            approval_request_id: "wallet-request:11111111111111111111111111111111".to_string(),
            request_sha256: format!("sha256:{}", hex::encode([0xac; 32])),
            chain_namespace: listing.package.chain_namespace.clone(),
            network: listing.package.network.clone(),
            to: "0x2222222222222222222222222222222222222222".to_string(),
            value: "0x1".to_string(),
            data: "0x".to_string(),
        },
        progress: RuntimeCustodyPurchaseProgress::Complete {
            terminal: RuntimeCustodyTerminalPurchaseRecord {
                chain_transaction,
                wallet_binding,
                chain_observation,
                access_evidence: RuntimeCustodyPurchaseAccessEvidenceRecord {
                    schema: "elastos.chain.protected-content-purchase-access/v1".to_string(),
                    request_id: "purchase-access:test".to_string(),
                    network: "esc-mainnet".to_string(),
                    chain_id: 20,
                    wallet: wallet_address_hex(wallet(7)),
                    content_access_id: listing.package.content_access_id.clone(),
                    has_access: true,
                    finalized_block_number: 44,
                    finalized_block_hash: format!("0x{}", hex::encode([0xad; 32])),
                    finalized_block_timestamp: now,
                    observed_at: now,
                },
                confirmed_at: now,
                bought_at: now,
            },
        },
        created_at: now,
        updated_at: now,
    };
    write_owner_only_bytes(
        &runtime_purchase_path(data_dir, principal_id, mint.draft().mint_id()),
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    record
}

#[cfg(unix)]
#[allow(
    clippy::too_many_arguments,
    reason = "test helper binds every fixture fact explicitly"
)]
fn persist_runtime_custody_active_viewer_for_purchase(
    data_dir: &Path,
    mint: &PersistedRuntimeMint,
    purchase: &RuntimeCustodyPurchaseRecord,
    proof_binding_id: &str,
    session_id: &str,
    grant_id: &str,
    handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    expires_at: u64,
) -> RuntimeViewerSession {
    let binding = super::derive_runtime_custody_session_binding(
        &purchase.principal_id,
        &purchase.profile_did,
        TEST_VIEWER_LAUNCH_ID,
        proof_binding_id,
        session_id,
        grant_id,
        mint.draft().mint_id(),
    )
    .unwrap();
    let session = RuntimeViewerSession::from_persisted_parts(
        digest(0x91),
        handle,
        mint.draft().encrypted_content().clone(),
        RightsActionV1::View,
        expires_at,
    )
    .unwrap();
    let record = super::RuntimeCustodyViewerRecord::from_active_session(
        &purchase.principal_id,
        &purchase.profile_did,
        mint.draft().mint_id(),
        &purchase.content_id,
        binding,
        &session,
        crate::auth::now_ts(),
    )
    .unwrap();
    super::persist_runtime_custody_viewer_record(
        data_dir,
        &purchase.principal_id,
        mint.draft().mint_id(),
        &record,
    )
    .unwrap();
    session
}

#[cfg(unix)]
#[allow(
    clippy::too_many_arguments,
    reason = "test helper binds every fixture fact explicitly"
)]
fn persist_runtime_custody_open_pending_viewer_for_purchase(
    data_dir: &Path,
    mint: &PersistedRuntimeMint,
    purchase: &RuntimeCustodyPurchaseRecord,
    proof_binding_id: &str,
    session_id: &str,
    grant_id: &str,
    handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    expires_at: u64,
) -> super::RuntimeCustodyViewerRecord {
    let binding = super::derive_runtime_custody_session_binding(
        &purchase.principal_id,
        &purchase.profile_did,
        TEST_VIEWER_LAUNCH_ID,
        proof_binding_id,
        session_id,
        grant_id,
        mint.draft().mint_id(),
    )
    .unwrap();
    let record = super::RuntimeCustodyViewerRecord::from_open_pending(
        super::RuntimeCustodyOpenPendingInput {
            principal_id: &purchase.principal_id,
            profile_did: &purchase.profile_did,
            mint_id: mint.draft().mint_id(),
            content_id: &purchase.content_id,
            runtime_session_binding: binding,
            audit_request_id: digest(0x91),
            viewer_session_handle: handle,
            expires_at,
            now: crate::auth::now_ts(),
        },
    )
    .unwrap();
    super::persist_runtime_custody_viewer_record(
        data_dir,
        &purchase.principal_id,
        mint.draft().mint_id(),
        &record,
    )
    .unwrap();
    record
}

#[cfg(unix)]
fn install_profile_authority_for_release_test(
    data_dir: &Path,
    principal_id: &str,
) -> (String, String, ProfileIdentityV1) {
    owner_only_dir(data_dir);
    fs::create_dir_all(data_dir.join("identity")).unwrap();
    fs::write(data_dir.join("identity/device.key"), [0x42; 32]).unwrap();
    let proof_binding_id =
        store_profile_signing_passkey(data_dir, principal_id, "credential-protected-content");
    crate::auth::store_test_principal_root_protection(data_dir, principal_id);
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let profile = crate::collaboration_profile_authority::update_profile_authority(
        data_dir,
        principal_id,
        &localhost_root,
        &proof_binding_id,
        "Protected Content Test",
        None,
        NOW,
    )
    .unwrap();
    let profile_key = crate::crypto::decode_did_key(&profile.document().profile_did).unwrap();
    let profile_identity =
        ProfileIdentityV1::from_public_key_bytes(profile_key.to_bytes()).unwrap();
    (proof_binding_id, localhost_root, profile_identity)
}

fn release_operation_assembly_input(
    profile: ProfileIdentityV1,
) -> super::RuntimeReleaseOperationAssemblyInput {
    let envelope = custody_envelope_for_media(0x11);
    let policy = policy_body();
    let binding = ProtectedContentBindingV1::new(
        envelope.manifest().encrypted_content().clone(),
        envelope.key_envelope_identity().unwrap(),
        policy.policy_identity().unwrap(),
        profile,
        wallet(7),
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap();
    let recipient_public_key = recipient_public_key(0x30);
    let recipient_identity = recipient_identity(0x30);
    let rights_request = {
        let request = elastos_protected_content_contracts::RightsRequestV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_identity.clone(),
            NOW,
            NOW + 180,
            ReplayNonce16::new([0x55; 16]),
        )
        .unwrap();
        let key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
        let (signature, recovery_id) = key
            .sign_prehash_recoverable(&elastos_auth::ethereum_signed_message_hash(
                &request.canonical_bytes().unwrap(),
            ))
            .unwrap();
        let mut signature_bytes = signature.to_bytes().to_vec();
        signature_bytes.push(recovery_id.to_byte());
        WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
    };
    let release_request = KeyReleaseRequestV1::new(
        binding.clone(),
        rights_request.request().request_hash().unwrap(),
        RightsActionV1::View,
        recipient_identity.clone(),
        NOW + 1,
        NOW + 50,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    super::RuntimeReleaseOperationAssemblyInput {
        rights_request,
        release_request,
        recipient_public_key,
        recipient_identity,
        policy_body: policy.clone(),
        evidence_request: RightsEvaluationEvidenceRequestV1::new(
            binding,
            policy.policy_identity().unwrap(),
        )
        .unwrap(),
        custody_epoch: signed_custody_epoch(),
        audit_request_id: RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
        issued_at: NOW + 2,
        expires_at: NOW + 40,
    }
}

#[cfg(unix)]
#[test]
fn profile_authorized_runtime_release_assembly_is_exact_and_side_effect_free() {
    let directory = tempfile::tempdir().unwrap();
    let principal_id = "person:local:protected-content-release";
    let (proof_binding_id, localhost_root, profile) =
        install_profile_authority_for_release_test(directory.path(), principal_id);
    let input = release_operation_assembly_input(profile);
    let profile_path = crate::collaboration_profile_authority::profile_authority_path(
        directory.path(),
        &localhost_root,
    )
    .unwrap();
    let profile_before = fs::read(&profile_path).unwrap();
    let profile_bundle = crate::auth::read_principal_root_object(
        directory.path(),
        principal_id,
        &localhost_root,
        &crate::collaboration_profile_authority::profile_authority_object_uri(&localhost_root),
        &profile_path,
    )
    .unwrap();
    let profile_signing_seed_hex = serde_json::from_slice::<Value>(&profile_bundle)
        .unwrap()
        .get("profile_signing_seed_hex")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();
    let device_before = fs::read(directory.path().join("identity/device.key")).unwrap();

    let operation = super::assemble_protected_content_runtime_release_operation(
        directory.path(),
        principal_id,
        &localhost_root,
        &proof_binding_id,
        input.clone(),
        NOW + 3,
    )
    .unwrap();
    let (device_key, _) = elastos_identity::derive_did(&[0x42; 32]);
    let expected_issuer =
        RuntimeOperationIssuerKeyV1::new(device_key.verifying_key().to_bytes()).unwrap();
    assert_eq!(
        operation.statement().runtime_operation_issuer(),
        expected_issuer
    );
    let verified = operation.verify(expected_issuer, NOW + 3);
    assert!(verified.is_ok(), "{verified:?}");
    let duplicate = super::assemble_protected_content_runtime_release_operation(
        directory.path(),
        principal_id,
        &localhost_root,
        &proof_binding_id,
        input,
        NOW + 3,
    )
    .unwrap();
    assert_eq!(
        operation.canonical_bytes().unwrap(),
        duplicate.canonical_bytes().unwrap()
    );
    assert_eq!(profile_before, fs::read(profile_path).unwrap());
    assert_eq!(
        device_before,
        fs::read(directory.path().join("identity/device.key")).unwrap()
    );
    assert!(!directory
        .path()
        .join("protected-content/runtime-release")
        .exists());
    let debug = format!("{operation:?}");
    assert!(!debug.contains("profile_signing_seed_hex"));
    assert!(!debug.contains(&profile_signing_seed_hex));
    assert!(!operation
        .canonical_bytes()
        .unwrap()
        .windows(profile_signing_seed_hex.len())
        .any(|window| window == profile_signing_seed_hex.as_bytes()));
    assert!(!debug.contains(&hex::encode([0x42; 32])));

    assert!(super::assemble_protected_content_runtime_release_operation(
        directory.path(),
        principal_id,
        &localhost_root,
        "proof:passkey:missing-protected-content-release",
        release_operation_assembly_input(profile),
        NOW + 3,
    )
    .is_err());

    let foreign_proof = store_profile_signing_passkey(
        directory.path(),
        "person:local:foreign-protected-content-release",
        "credential-foreign-protected-content",
    );
    assert!(super::assemble_protected_content_runtime_release_operation(
        directory.path(),
        principal_id,
        &localhost_root,
        &foreign_proof,
        release_operation_assembly_input(profile),
        NOW + 3,
    )
    .is_err());
    crate::auth::revoke_passkey_binding(directory.path(), &proof_binding_id, NOW + 4).unwrap();
    assert!(super::assemble_protected_content_runtime_release_operation(
        directory.path(),
        principal_id,
        &localhost_root,
        &proof_binding_id,
        release_operation_assembly_input(profile),
        NOW + 4,
    )
    .is_err());
}

#[cfg(unix)]
#[test]
fn profile_authorized_runtime_release_assembly_fails_without_required_authority() {
    let unprotected = tempfile::tempdir().unwrap();
    let unprotected_principal = "person:local:unprotected-profile-release";
    owner_only_dir(unprotected.path());
    fs::create_dir_all(unprotected.path().join("identity")).unwrap();
    fs::write(unprotected.path().join("identity/device.key"), [0x42; 32]).unwrap();
    let unprotected_proof = store_profile_signing_passkey(
        unprotected.path(),
        unprotected_principal,
        "credential-unprotected-profile",
    );
    let other_profile = ProfileIdentityV1::from_public_key_bytes(
        SigningKey::from_bytes(&[0x26; 32])
            .verifying_key()
            .to_bytes(),
    )
    .unwrap();
    let error = super::assemble_protected_content_runtime_release_operation(
        unprotected.path(),
        unprotected_principal,
        &crate::auth::principal_localhost_root(unprotected_principal),
        &unprotected_proof,
        release_operation_assembly_input(other_profile),
        NOW + 3,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("protected principal root is required"));

    let missing_profile = tempfile::tempdir().unwrap();
    let missing_profile_principal = "person:local:missing-profile-release";
    owner_only_dir(missing_profile.path());
    fs::create_dir_all(missing_profile.path().join("identity")).unwrap();
    fs::write(
        missing_profile.path().join("identity/device.key"),
        [0x42; 32],
    )
    .unwrap();
    let missing_profile_proof = store_profile_signing_passkey(
        missing_profile.path(),
        missing_profile_principal,
        "credential-missing-profile",
    );
    crate::auth::store_test_principal_root_protection(
        missing_profile.path(),
        missing_profile_principal,
    );
    assert!(super::assemble_protected_content_runtime_release_operation(
        missing_profile.path(),
        missing_profile_principal,
        &crate::auth::principal_localhost_root(missing_profile_principal),
        &missing_profile_proof,
        release_operation_assembly_input(other_profile),
        NOW + 3,
    )
    .is_err());

    let missing_device = tempfile::tempdir().unwrap();
    let missing_device_principal = "person:local:missing-device-release";
    let (proof_binding_id, localhost_root, profile) =
        install_profile_authority_for_release_test(missing_device.path(), missing_device_principal);
    fs::remove_file(missing_device.path().join("identity/device.key")).unwrap();
    assert!(super::assemble_protected_content_runtime_release_operation(
        missing_device.path(),
        missing_device_principal,
        &localhost_root,
        &proof_binding_id,
        release_operation_assembly_input(profile),
        NOW + 3,
    )
    .is_err());

    let mismatch = tempfile::tempdir().unwrap();
    let mismatch_principal = "person:local:mismatched-profile-release";
    let (proof_binding_id, localhost_root, _) =
        install_profile_authority_for_release_test(mismatch.path(), mismatch_principal);
    assert!(super::assemble_protected_content_runtime_release_operation(
        mismatch.path(),
        mismatch_principal,
        &localhost_root,
        &proof_binding_id,
        release_operation_assembly_input(other_profile),
        NOW + 3,
    )
    .is_err());
}

fn make_signed_runtime_release_operation_for_envelope_and_seed(
    seed: u8,
    envelope: &CustodyEnvelopeV1,
) -> SignedRuntimeReleaseOperationV1 {
    make_signed_runtime_release_operation_for_envelope_and_epoch_at(
        seed,
        envelope,
        signed_custody_epoch(),
        NOW,
    )
}

fn make_signed_runtime_release_operation_for_envelope_and_epoch_at(
    seed: u8,
    envelope: &CustodyEnvelopeV1,
    custody_epoch: SignedCustodyEpochV1,
    now: u64,
) -> SignedRuntimeReleaseOperationV1 {
    make_signed_runtime_release_operation_for_envelope_and_epoch_and_recipient_at(
        seed,
        envelope,
        custody_epoch,
        recipient_public_key(0x30),
        recipient_identity(0x30),
        RuntimeReleaseAuditIdV1::new(digest(0x91 ^ seed)).unwrap(),
        now,
    )
}

fn make_signed_runtime_release_operation_for_envelope_and_epoch_and_recipient_at(
    seed: u8,
    envelope: &CustodyEnvelopeV1,
    custody_epoch: SignedCustodyEpochV1,
    recipient_public: RecipientPublicKeyBytesV1,
    recipient_identity: RecipientKeyIdentityV1,
    audit_request_id: RuntimeReleaseAuditIdV1,
    now: u64,
) -> SignedRuntimeReleaseOperationV1 {
    let runtime_key = SigningKey::from_bytes(&[seed; 32]);
    let binding = binding_for_envelope(envelope);
    let rights_request = {
        let request = elastos_protected_content_contracts::RightsRequestV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_identity.clone(),
            now.saturating_sub(5),
            now + 180,
            ReplayNonce16::new([0x55; 16]),
        )
        .unwrap();
        let key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
        let (signature, recovery_id) = key
            .sign_prehash_recoverable(&elastos_auth::ethereum_signed_message_hash(
                &request.canonical_bytes().unwrap(),
            ))
            .unwrap();
        let mut signature_bytes = signature.to_bytes().to_vec();
        signature_bytes.push(recovery_id.to_byte());
        WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
    };
    let release_request = KeyReleaseRequestV1::new(
        binding.clone(),
        rights_request.request().request_hash().unwrap(),
        RightsActionV1::View,
        rights_request.request().recipient().clone(),
        now.saturating_sub(4),
        now + 56,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    let profile = SigningKey::from_bytes(&[0x26; 32]);
    let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
        binding.clone(),
        RightsActionV1::View,
        recipient_public,
        rights_request.request().recipient().clone(),
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        now.saturating_sub(5),
        now + 120,
    )
    .unwrap();
    let authorization = SignedRecipientKeyAuthorizationV1::new(
        authorization_statement.clone(),
        profile
            .sign(&authorization_statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let policy = policy_body();
    let evidence_request =
        elastos_protected_content_contracts::RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
        )
        .unwrap();
    let statement = RuntimeReleaseOperationStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        rights_request,
        release_request,
        recipient_public,
        authorization,
        policy,
        evidence_request,
        custody_epoch,
        audit_request_id,
        now.saturating_sub(3),
        now + 55,
    )
    .unwrap();
    SignedRuntimeReleaseOperationV1::new(
        statement.clone(),
        runtime_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn wallet_request_response_for_release_at(
    operation: &SignedRuntimeReleaseOperationV1,
    profile: &str,
    account_id: &str,
    request_id: &str,
    now: u64,
) -> (Vec<u8>, Vec<u8>) {
    wallet_request_response_for_release_context_at(
        operation,
        profile,
        "runtime-session:alpha",
        Some("proof:alpha"),
        "grant:alpha",
        "launch:alpha",
        account_id,
        request_id,
        now,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "test helper binds every fixture fact explicitly"
)]
fn wallet_request_response_for_release_context_at(
    operation: &SignedRuntimeReleaseOperationV1,
    profile: &str,
    runtime_session_id: &str,
    proof_binding: Option<&str>,
    grant_id: &str,
    launch_id: &str,
    account_id: &str,
    request_id: &str,
    now: u64,
) -> (Vec<u8>, Vec<u8>) {
    let context = VerifiedWalletInvocationContext::new(
        profile,
        runtime_session_id,
        proof_binding.map(str::to_string),
        grant_id,
        "runtime",
        launch_id,
    )
    .unwrap();
    let request = WalletProviderRequestV2::new(
        &context,
        request_id,
        now,
        now + 120,
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            account_id: account_id.to_string(),
            canonical_rights_request_hex: hex::encode(
                operation
                    .statement()
                    .rights_request()
                    .request()
                    .canonical_bytes()
                    .unwrap(),
            ),
            reason: "Open protected content".to_string(),
        },
    )
    .unwrap();
    let result = ProtectedContentRightsSignatureResultV1::new(
        account_id,
        wallet_address_hex(
            operation
                .statement()
                .rights_request()
                .request()
                .binding()
                .wallet(),
        ),
        hex::encode(
            operation
                .statement()
                .rights_request()
                .canonical_bytes()
                .unwrap(),
        ),
    )
    .unwrap();
    let response = WalletProviderResponseV2::for_request(
        &request,
        WalletResultV2::Ok {
            data: serde_json::to_value(result).unwrap(),
        },
    );
    (
        serde_json::to_vec(&request).unwrap(),
        serde_json::to_vec(&response).unwrap(),
    )
}

fn signed_runtime_custody_provisioning_at(
    record: &CustodyNodeProvisioningRecordV1,
    runtime_seed: u8,
    now: u64,
) -> SignedRuntimeCustodyProvisioningV1 {
    let runtime_key = SigningKey::from_bytes(&[runtime_seed; 32]);
    let statement = RuntimeCustodyProvisioningStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        record.record_identity().unwrap(),
        RuntimeCustodyProvisioningIdV1::new(digest(0xa5)).unwrap(),
        now.saturating_sub(5),
        now + 40,
    )
    .unwrap();
    SignedRuntimeCustodyProvisioningV1::new(
        statement.clone(),
        runtime_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn chain_evidence_for_request_at(
    request: &RightsProviderRequestV1,
    now_unix_seconds: u64,
    evidence_now: u64,
    has_access: bool,
) -> Value {
    let operation = request.signed_runtime_release_operation().unwrap();
    let validated = ValidatedRightsProviderRequestV1::decode_and_validate_at(
        &request.to_json_vec().unwrap(),
        operation.statement().runtime_operation_issuer(),
        now_unix_seconds,
    )
    .unwrap();
    let authenticated = validated.authenticated_runtime_release_operation();
    let policy = authenticated.statement().policy_body();
    let evidence_request = authenticated.statement().evidence_request();
    let evidence_issued_at = evidence_now.saturating_sub(1);
    let evidence = RightsEvaluationEvidenceV1::new(
        authenticated.operation_hash(),
        authenticated.release_request_hash(),
        evidence_request.binding().clone(),
        evidence_request.policy_identity().clone(),
        evidence_request.binding().wallet(),
        policy.chain_id(),
        100,
        digest(0x88),
        has_access,
        evidence_issued_at,
        evidence_issued_at + MAX_RIGHTS_EVIDENCE_LIFETIME_SECS,
    )
    .unwrap();
    let bytes = evidence.canonical_bytes().unwrap();
    json!({
        "schema": "elastos.chain.protected-content-rights-evidence/v1",
        "chain_id": evidence.observed_chain_id(),
        "finalized_block_number": evidence.finalized_block_number(),
        "finalized_block_hash": format!(
            "0x{}",
            hex::encode(evidence.finalized_block_hash().as_bytes())
        ),
        "rights_evaluation_evidence": format!("0x{}", hex::encode(&bytes)),
        "rights_evaluation_evidence_hash": format!(
            "0x{}",
            hex::encode(evidence.canonical_hash().unwrap().as_bytes())
        ),
    })
}

fn make_signed_node_rights_decision(
    operation: &SignedRuntimeReleaseOperationV1,
    node_seed: u8,
    decision: RightsDecisionV1,
) -> SignedNodeRightsDecisionV1 {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), NOW + 3)
        .unwrap();
    let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.rights_request_hash(),
        authenticated.binding().clone(),
        authenticated.action(),
        node_public_key(node_seed),
        decision,
        digest(0x80 ^ node_seed),
        NOW + 4,
        NOW + 40,
    )
    .unwrap();
    SignedNodeRightsDecisionV1::new(
        statement.clone(),
        node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn make_signed_node_contribution(
    operation: &SignedRuntimeReleaseOperationV1,
    node_seed: u8,
) -> SignedNodeContributionV1 {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), NOW + 5)
        .unwrap();
    let decision =
        make_signed_node_rights_decision(operation, node_seed, RightsDecisionV1::Allowed);
    let sealed =
        RecipientSealedContributionV1::new(authenticated.recipient().clone(), vec![node_seed; 96])
            .unwrap();
    let statement = NodeContributionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.binding().clone(),
        decision,
        sealed,
        NOW + 5,
        NOW + 40,
    )
    .unwrap();
    SignedNodeContributionV1::new(
        statement.clone(),
        node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn make_signed_terminal_receipt(
    operation: &SignedRuntimeReleaseOperationV1,
    contributions: &[SignedNodeContributionV1],
    issuer_seed: u8,
) -> SignedTerminalReceiptV1 {
    make_signed_terminal_receipt_at(operation, contributions, issuer_seed, NOW + 6)
}

fn make_signed_terminal_receipt_at(
    operation: &SignedRuntimeReleaseOperationV1,
    contributions: &[SignedNodeContributionV1],
    issuer_seed: u8,
    now: u64,
) -> SignedTerminalReceiptV1 {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), now)
        .unwrap();
    let node_set = operation
        .statement()
        .custody_epoch()
        .statement()
        .node_set()
        .unwrap();
    let verified_contributions = contributions
        .iter()
        .map(|contribution| {
            authenticated
                .verify_node_contribution(contribution, &node_set, now)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let refs = verified_contributions
        .iter()
        .map(NodeContributionRefV1::from)
        .collect::<Vec<_>>();
    let expires_at = contributions
        .iter()
        .map(|contribution| contribution.statement().expires_at())
        .min()
        .unwrap_or_else(|| now.saturating_add(34))
        .min(now.saturating_add(34));
    let issuer_key = SigningKey::from_bytes(&[issuer_seed; 32]);
    let statement = TerminalReceiptStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.binding().clone(),
        TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        KeyReleaseOutcomeV1::Released,
        refs,
        now,
        expires_at,
    )
    .unwrap();
    SignedTerminalReceiptV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn provisioning_record_for_selected_node(
    envelope: &CustodyEnvelopeV1,
    selected_node_public_key: NodePublicKey,
) -> CustodyNodeProvisioningRecordV1 {
    CustodyNodeProvisioningRecordV1::new(
        envelope.key_envelope_identity().unwrap(),
        envelope.manifest().clone(),
        selected_node_public_key,
        envelope
            .stored_share_for_node(selected_node_public_key)
            .unwrap()
            .clone(),
    )
    .unwrap()
}

fn opaque_handle(seed: u8) -> [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
    let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
    bytes[0] = seed.max(1);
    bytes[31] = seed ^ 0x5a;
    bytes
}

fn decrypt_prepare_request() -> DecryptProviderRequestV1 {
    let envelope = custody_envelope_for_media(0x11);
    DecryptProviderRequestV1::new_prepare_recipient(
        &binding_for_envelope(&envelope),
        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
        RightsActionV1::View,
        runtime_operation_issuer_for_seed(0x42),
        NOW,
        NOW + 50,
    )
    .unwrap()
}

fn decrypt_open_fixture() -> (
    DecryptProviderRequestV1,
    RuntimeReleaseAuditIdV1,
    [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    Vec<u8>,
    Vec<Vec<u8>>,
) {
    let envelope = custody_envelope_for_media(0x11);
    let operation = make_signed_runtime_release_operation_for_envelope_and_seed(0x42, &envelope);
    let contributions = vec![
        make_signed_node_contribution(&operation, 1),
        make_signed_node_contribution(&operation, 2),
    ];
    let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61);
    let handle = opaque_handle(0x21);
    let (init_segment, encrypted_segments) = media_components(0x11);
    let media = media_identity(0x11);
    let request = DecryptProviderRequestV1::new_open_viewer_session(
        handle,
        &operation,
        terminal.statement().issuer(),
        envelope.manifest().content_key_commitment(),
        &media,
        &init_segment,
        &contributions,
        &terminal,
    )
    .unwrap();
    (
        request,
        operation.statement().audit_request_id(),
        handle,
        init_segment,
        encrypted_segments,
    )
}

fn assert_exact_runtime_decrypt_invocation(
    recorded: &Value,
    expected_op: &str,
    expected_request: &Value,
) {
    assert_eq!(recorded["op"], expected_op);
    assert_eq!(
        recorded["_runtime_invocation"]["source"],
        RUNTIME_PROVIDER_ID
    );
    assert_eq!(
        recorded["_runtime_invocation"]["target"],
        PROTECTED_CONTENT_DECRYPT_PROVIDER_ID
    );
    assert_eq!(
        recorded["_runtime_invocation"]["transport"],
        "runtime-local-provider-plane"
    );
    assert_eq!(recorded["_runtime_invocation"]["carrier"], Value::Null);
    assert!(recorded.get("route").is_none());
    assert!(recorded.get("host").is_none());
    assert!(recorded.get("port").is_none());
    assert!(recorded.get("url").is_none());
    let mut stripped = recorded.clone();
    stripped
        .as_object_mut()
        .unwrap()
        .remove("_runtime_invocation");
    assert_eq!(&stripped, expected_request);
}

#[cfg(unix)]
struct ProcessCustodyNodeFixture {
    registry: Arc<ProviderRegistry>,
    adapter: RuntimeCustodyRegistryAdapter,
    provisioned: ProvisionedCustodyProviderPublicKeys,
    owner_state_root: Digest32,
}

#[cfg(unix)]
fn provisioned_process_custody_node_for_issuer(
    binary: &Path,
    temp_root: &Path,
    dir_name: &str,
    runtime_issuer: RuntimeOperationIssuerKeyV1,
    owner_state_root: Digest32,
) -> ProcessCustodyNodeFixture {
    let data_dir = temp_root.join(dir_name);
    owner_only_dir(&data_dir);
    let state_root = inactive_custody_state_root(&data_dir);
    owner_only_dir(state_root.parent().unwrap());
    let provisioned =
        provision_custody_node_public_receipt_for_issuer(binary, &state_root, runtime_issuer);
    let registry = Arc::new(ProviderRegistry::new());
    ProcessCustodyNodeFixture {
        adapter: RuntimeCustodyRegistryAdapter::new(
            registry.clone(),
            elastos_runtime::provider::ProviderInvocationTransport::Local,
        ),
        registry,
        provisioned,
        owner_state_root,
    }
}

#[cfg(unix)]
fn provision_custody_node_public_receipt_for_issuer(
    binary: &Path,
    state_root: &Path,
    runtime_issuer: RuntimeOperationIssuerKeyV1,
) -> ProvisionedCustodyProviderPublicKeys {
    let issuer_hex = format!("0x{}", hex::encode(runtime_issuer.as_bytes()));
    let output = Command::new(binary)
        .args([
            "provision",
            "--base-path",
            state_root.to_str().unwrap(),
            "--trusted-runtime-issuer",
            &issuer_hex,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status={:?} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    parse_and_verify_provisioning_output(&response, runtime_issuer).unwrap()
}

#[cfg(unix)]
fn provisioned_process_custody_node(
    binary: &Path,
    temp_root: &Path,
    dir_name: &str,
    runtime_issuer_seed: u8,
    owner_state_root: Digest32,
) -> ProcessCustodyNodeFixture {
    let data_dir = temp_root.join(dir_name);
    owner_only_dir(&data_dir);
    let state_root = inactive_custody_state_root(&data_dir);
    owner_only_dir(state_root.parent().unwrap());
    let provisioned =
        provision_custody_node_public_receipt(binary, &state_root, runtime_issuer_seed);
    let registry = Arc::new(ProviderRegistry::new());
    ProcessCustodyNodeFixture {
        adapter: RuntimeCustodyRegistryAdapter::new(
            registry.clone(),
            elastos_runtime::provider::ProviderInvocationTransport::Local,
        ),
        registry,
        provisioned,
        owner_state_root,
    }
}

#[cfg(unix)]
async fn provision_selected_node_share(
    fixture: &ProcessCustodyNodeFixture,
    envelope: &CustodyEnvelopeV1,
    runtime_seed: u8,
    now: u64,
) {
    let provisioning_record =
        provisioning_record_for_selected_node(envelope, fixture.provisioned.node_public_key);
    let provisioning =
        signed_runtime_custody_provisioning_at(&provisioning_record, runtime_seed, now);
    let request =
        CustodyProviderRequestV1::new_provision_node_share(&provisioning_record, &provisioning)
            .unwrap();
    let response = fixture
        .adapter
        .provision_node_share(&request)
        .await
        .unwrap();
    assert_eq!(
        response.provisioned_record_identity().unwrap(),
        provisioning_record.record_identity().unwrap()
    );
}

#[cfg(unix)]
fn runtime_verified_purchase_effect_for_mint(
    mint: &PersistedRuntimeMint,
    principal_id: &str,
    account_id: &str,
    approval_request_id: &str,
    tx_byte: u8,
    now: u64,
) -> RuntimeVerifiedPurchaseEffect {
    RuntimeVerifiedPurchaseEffect::new(
        RuntimeProtectedContentPurchaseIntent::new(
            mint.draft().mint_id(),
            mint.draft().encrypted_content().clone(),
            mint.draft().key_envelope().clone(),
            mint.draft().policy().clone(),
            RightsActionV1::View,
            "eip155:20",
            "esc-mainnet",
            "0x2222222222222222222222222222222222222222",
            "0x1",
            "0x",
        )
        .unwrap(),
        RuntimePurchaseEffectAuthority::new(
            principal_id,
            account_id,
            wallet_address_hex(wallet(7)),
            approval_request_id,
        )
        .unwrap(),
        ValidatedChainOutcomeBindingV1::ManagedSigned {
            signed_transaction_sha256: format!("sha256:{}", hex::encode([tx_byte ^ 1; 32])),
        },
        format!("0x{}", hex::encode([tx_byte; 32])),
        json!({
            "schema": "elastos.chain.broadcast_receipt/v1",
            "network": "esc-mainnet",
        }),
        now,
    )
    .unwrap()
}

fn ok_provider_response(value: serde_json::Value) -> serde_json::Value {
    json!({
        "status": "ok",
        "data": value,
    })
}

fn ok_typed_protect_provider_response(response: ProtectProviderResponseV1) -> serde_json::Value {
    ok_provider_response(serde_json::from_slice(&response.to_json_vec().unwrap()).unwrap())
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "recording provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![self.name]
    }

    fn name(&self) -> &'static str {
        self.name
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        Ok(self.response.clone())
    }
}

#[async_trait::async_trait]
impl Provider for SequencedProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "sequenced provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![self.name]
    }

    fn name(&self) -> &'static str {
        self.name
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        self.responses.lock().await.pop_front().unwrap_or_else(|| {
            Err(ProviderError::Provider(
                "sequenced provider response queue is exhausted".to_string(),
            ))
        })
    }
}

#[async_trait::async_trait]
impl Provider for ProcessChainEvidenceProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "process chain evidence provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![CHAIN_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        CHAIN_PROVIDER_ID
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        self.requests.lock().await.push(request.clone());
        let expected = elastos_protected_content_rights::chain_rights_evidence_request(
            &self
                .expected_request
                .signed_runtime_release_operation()
                .map_err(|_| {
                    ProviderError::Provider("expected rights request is invalid".to_string())
                })?,
        )
        .map_err(|_| ProviderError::Provider("expected rights request is invalid".to_string()))?;
        if request["_runtime_invocation"]["source"]
            != Value::String(RUNTIME_PROVIDER_ID.to_string())
            || request["_runtime_invocation"]["target"]
                != Value::String(CHAIN_PROVIDER_ID.to_string())
            || request["_runtime_invocation"]["op"]
                != Value::String(CHAIN_RIGHTS_EVIDENCE_OP.to_string())
            || request["_runtime_invocation"]["transport"]
                != Value::String("runtime-local-provider-plane".to_string())
            || request["_runtime_invocation"]["carrier"] != Value::Null
        {
            return Err(ProviderError::Provider(
                "chain evidence runtime envelope did not match the expected invocation".to_string(),
            ));
        }
        let mut inner_request = request.clone();
        let Some(inner_object) = inner_request.as_object_mut() else {
            return Err(ProviderError::Provider(
                "chain evidence request was not an object".to_string(),
            ));
        };
        inner_object.remove("_runtime_invocation");
        if inner_request != expected {
            return Err(ProviderError::Provider(
                "chain evidence request did not match the expected release operation".to_string(),
            ));
        }
        let evidence_now = crate::auth::now_ts();
        Ok(ok_provider_response(chain_evidence_for_request_at(
            &self.expected_request,
            evidence_now,
            evidence_now,
            self.has_access,
        )))
    }
}

#[cfg(unix)]
#[async_trait::async_trait]
impl Provider for TestMediaPreparationProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "test media provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![MEDIA_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        MEDIA_PROVIDER_ID
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let operation_id = request
            .get("operation_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing operation id".to_string()))?
            .to_string();
        if request["op"] != "prepare"
            || request["_runtime_invocation"]["source"] != RUNTIME_PROVIDER_ID
            || request["_runtime_invocation"]["target"] != MEDIA_PROVIDER_ID
            || request["_runtime_invocation"]["op"] != "prepare"
            || request["_runtime_invocation"]["transport"] != "runtime-local-provider-plane"
            || request["_runtime_invocation"]["carrier"] != Value::Null
        {
            return Err(ProviderError::Provider(
                "media preparation invocation authority is invalid".to_string(),
            ));
        }
        self.requests.lock().await.push(operation_id.clone());
        match self.response {
            TestMediaPreparationResponse::Prepared => {
                let prepared_root = self.staging_root.join(operation_id).join("prepared");
                owner_only_dir(&prepared_root);
                owner_only_dir(&prepared_root.join("segments"));
                let (init, segments) = clear_media_components(0x61);
                fs::write(prepared_root.join("init.mp4"), init).unwrap();
                fs::set_permissions(
                    prepared_root.join("init.mp4"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
                for (index, segment) in segments.iter().enumerate() {
                    let path = prepared_root
                        .join("segments")
                        .join(format!("{index:08}.m4s"));
                    fs::write(&path, segment).unwrap();
                    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
                }
                Ok(ok_provider_response(json!({
                    "schema": "elastos.media-provider.prepared-media/v1",
                    "mime_type": "video/mp4",
                    "codecs": "avc1.640028",
                })))
            }
            TestMediaPreparationResponse::SettledFailure => Ok(json!({
                "status": "error",
                "code": "internal_error",
                "message": "media preparation failed",
                "data": {"operation_settled": true},
            })),
            TestMediaPreparationResponse::UnknownSettlement => Err(ProviderError::Provider(
                "media preparation transport failed".to_string(),
            )),
        }
    }
}

#[cfg(unix)]
fn owner_only_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
fn protected_content_root(data_dir: &Path) -> PathBuf {
    data_dir.join("protected-content")
}

#[cfg(unix)]
fn custody_composition_config_path(data_dir: &Path) -> PathBuf {
    protected_content_root(data_dir).join("custody-composition.json")
}

#[cfg(unix)]
fn chain_provider_config_path(data_dir: &Path) -> PathBuf {
    protected_content_root(data_dir).join("chain-provider.json")
}

#[cfg(unix)]
fn write_chain_provider_config(data_dir: &Path, config: &Value) {
    owner_only_dir(&protected_content_root(data_dir));
    fs::write(
        chain_provider_config_path(data_dir),
        serde_json::to_vec(config).unwrap(),
    )
    .unwrap();
    fs::set_permissions(
        chain_provider_config_path(data_dir),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}

#[cfg(unix)]
fn write_media_prerequisite(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn media_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("protected-content/media-provider/config.json")
}

#[cfg(unix)]
fn prepare_test_media_prerequisite(data_dir: &Path, root: &Path) {
    owner_only_dir(root);
    write_media_prerequisite(&root.join("ffmpeg"), b"ffmpeg-private-test", 0o700);
    write_media_prerequisite(&root.join("ffprobe"), b"ffprobe-private-test", 0o700);
    prepare_runtime_media_provider_prerequisite_with_path(data_dir, root.as_os_str()).unwrap();
}

#[cfg(unix)]
#[test]
fn media_prerequisite_import_is_bounded_private_idempotent_and_never_executes_tools() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let source = temp.path().join("source");
    let marker = temp.path().join("executed");
    owner_only_dir(&source);
    write_media_prerequisite(
        &source.join("ffmpeg"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()).as_bytes(),
        0o700,
    );
    write_media_prerequisite(
        &source.join("ffprobe"),
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()).as_bytes(),
        0o700,
    );

    prepare_runtime_media_provider_prerequisite_with_path(&data_dir, source.as_os_str()).unwrap();
    let config_path = media_config_path(&data_dir);
    let original_config = fs::read(&config_path).unwrap();
    let original_ffmpeg =
        fs::read(data_dir.join("protected-content/media-provider/tools/ffmpeg")).unwrap();
    prepare_runtime_media_provider_prerequisite_with_path(&data_dir, std::ffi::OsStr::new(""))
        .unwrap();

    assert!(!marker.exists());
    assert_eq!(fs::read(&config_path).unwrap(), original_config);
    assert_eq!(
        fs::read(data_dir.join("protected-content/media-provider/tools/ffmpeg")).unwrap(),
        original_ffmpeg
    );
    let config: Value = serde_json::from_slice(&original_config).unwrap();
    assert_eq!(config["schema"], MEDIA_PROVIDER_CONFIG_SCHEMA_V1);
    assert_eq!(config["timeout_ms"], MEDIA_PROVIDER_TIMEOUT_MS_V1);
    assert_eq!(config["max_input_bytes"], MEDIA_PROVIDER_MAX_INPUT_BYTES_V1);
    assert_eq!(config["max_segment_count"], 512);
    assert_eq!(config["max_total_output_bytes"], 2u64 << 30);
    assert_eq!(
        fs::symlink_metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(fs::symlink_metadata(&config_path).unwrap().nlink(), 1);
    for name in ["ffmpeg", "ffprobe"] {
        let metadata = fs::symlink_metadata(
            data_dir
                .join("protected-content/media-provider/tools")
                .join(name),
        )
        .unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o500);
        assert_eq!(metadata.nlink(), 1);
    }
    let bridge = load_runtime_media_provider_bridge_config(&data_dir)
        .unwrap()
        .expect("generated config must survive restart validation");
    assert_eq!(bridge.extra["provider_id"], "media-provider");

    let mut bounded: Value = serde_json::from_slice(&original_config).unwrap();
    bounded["timeout_ms"] = json!(5_000);
    bounded["max_input_bytes"] = json!(1u64 << 20);
    let bounded_bytes = serde_json::to_vec_pretty(&bounded).unwrap();
    fs::write(&config_path, &bounded_bytes).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    prepare_runtime_media_provider_prerequisite_with_path(&data_dir, std::ffi::OsStr::new(""))
        .unwrap();
    assert_eq!(fs::read(config_path).unwrap(), bounded_bytes);
}

#[cfg(unix)]
#[test]
fn media_prerequisite_rejects_missing_unsafe_and_oversized_tools_before_config() {
    let temp = tempfile::tempdir().unwrap();
    let missing_data = temp.path().join("missing-data");
    let missing = temp.path().join("missing-tools");
    owner_only_dir(&missing);
    write_media_prerequisite(&missing.join("ffmpeg"), b"ffmpeg", 0o700);
    let error =
        prepare_runtime_media_provider_prerequisite_with_path(&missing_data, missing.as_os_str())
            .unwrap_err();
    assert!(error.to_string().contains("ffprobe"));
    assert!(!media_config_path(&missing_data).exists());

    let unsafe_data = temp.path().join("unsafe-data");
    let unsafe_tools = temp.path().join("unsafe-tools");
    owner_only_dir(&unsafe_tools);
    write_media_prerequisite(&unsafe_tools.join("ffmpeg"), b"ffmpeg", 0o720);
    write_media_prerequisite(&unsafe_tools.join("ffprobe"), b"ffprobe", 0o700);
    assert!(prepare_runtime_media_provider_prerequisite_with_path(
        &unsafe_data,
        unsafe_tools.as_os_str()
    )
    .unwrap_err()
    .to_string()
    .contains("ffmpeg prerequisite"));
    assert!(!media_config_path(&unsafe_data).exists());

    let oversized_data = temp.path().join("oversized-data");
    let oversized_tools = temp.path().join("oversized-tools");
    owner_only_dir(&oversized_tools);
    let oversized = fs::File::create(oversized_tools.join("ffmpeg")).unwrap();
    oversized.set_len((512 * 1024 * 1024) + 1).unwrap();
    fs::set_permissions(
        oversized_tools.join("ffmpeg"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    write_media_prerequisite(&oversized_tools.join("ffprobe"), b"ffprobe", 0o700);
    assert!(prepare_runtime_media_provider_prerequisite_with_path(
        &oversized_data,
        oversized_tools.as_os_str()
    )
    .unwrap_err()
    .to_string()
    .contains("exceeds bounds"));
    assert!(!media_config_path(&oversized_data).exists());

    let non_file_data = temp.path().join("non-file-data");
    let non_file_tools = temp.path().join("non-file-tools");
    owner_only_dir(&non_file_tools);
    owner_only_dir(&non_file_tools.join("ffmpeg"));
    write_media_prerequisite(&non_file_tools.join("ffprobe"), b"ffprobe", 0o700);
    assert!(prepare_runtime_media_provider_prerequisite_with_path(
        &non_file_data,
        non_file_tools.as_os_str()
    )
    .unwrap_err()
    .to_string()
    .contains("ffmpeg prerequisite"));

    let unsafe_parent_data = temp.path().join("unsafe-parent-data");
    let unsafe_parent_tools = temp.path().join("unsafe-parent-tools");
    owner_only_dir(&unsafe_parent_tools);
    write_media_prerequisite(&unsafe_parent_tools.join("ffmpeg"), b"ffmpeg", 0o700);
    write_media_prerequisite(&unsafe_parent_tools.join("ffprobe"), b"ffprobe", 0o700);
    fs::set_permissions(&unsafe_parent_tools, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(prepare_runtime_media_provider_prerequisite_with_path(
        &unsafe_parent_data,
        unsafe_parent_tools.as_os_str()
    )
    .unwrap_err()
    .to_string()
    .contains("prerequisite parent is unsafe"));

    let linked_data = temp.path().join("linked-data");
    let linked_tools = temp.path().join("linked-tools");
    owner_only_dir(&linked_tools);
    write_media_prerequisite(&linked_tools.join("ffmpeg"), b"ffmpeg", 0o700);
    fs::hard_link(
        linked_tools.join("ffmpeg"),
        linked_tools.join("ffmpeg-copy"),
    )
    .unwrap();
    write_media_prerequisite(&linked_tools.join("ffprobe"), b"ffprobe", 0o700);
    assert!(prepare_runtime_media_provider_prerequisite_with_path(
        &linked_data,
        linked_tools.as_os_str()
    )
    .unwrap_err()
    .to_string()
    .contains("ffmpeg prerequisite"));
}

#[cfg(unix)]
#[test]
fn media_prerequisite_rejects_links_modes_paths_and_unknown_config_fields_without_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let data_dir = temp.path().join("data");
    prepare_test_media_prerequisite(&data_dir, &source);
    let config_path = media_config_path(&data_dir);
    let valid = fs::read(&config_path).unwrap();

    let mut unknown: Value = serde_json::from_slice(&valid).unwrap();
    unknown["credential"] = json!("private-value");
    fs::write(&config_path, serde_json::to_vec(&unknown).unwrap()).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    let error = load_runtime_media_provider_bridge_config(&data_dir).unwrap_err();
    assert!(!error.to_string().contains("private-value"));
    assert_eq!(
        fs::read(&config_path).unwrap(),
        serde_json::to_vec(&unknown).unwrap()
    );

    fs::write(&config_path, &valid).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(load_runtime_media_provider_bridge_config(&data_dir).is_err());
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    let config_copy = data_dir.join("protected-content/media-provider/config-copy");
    fs::hard_link(&config_path, &config_copy).unwrap();
    assert!(load_runtime_media_provider_bridge_config(&data_dir).is_err());
    fs::remove_file(config_copy).unwrap();

    let mut escaped: Value = serde_json::from_slice(&valid).unwrap();
    escaped["ffmpeg_path"] = json!("/private/tmp/ffmpeg");
    fs::write(&config_path, serde_json::to_vec(&escaped).unwrap()).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(load_runtime_media_provider_bridge_config(&data_dir).is_err());

    fs::write(&config_path, &valid).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    let ffmpeg = data_dir.join("protected-content/media-provider/tools/ffmpeg");
    let ffmpeg_target = data_dir.join("protected-content/media-provider/tools/ffmpeg-target");
    fs::rename(&ffmpeg, &ffmpeg_target).unwrap();
    std::os::unix::fs::symlink(&ffmpeg_target, &ffmpeg).unwrap();
    assert!(load_runtime_media_provider_bridge_config(&data_dir).is_err());

    let symlink_data = temp.path().join("symlink-data");
    owner_only_dir(&symlink_data);
    owner_only_dir(&symlink_data.join("protected-content/media-provider"));
    std::os::unix::fs::symlink(&config_path, media_config_path(&symlink_data)).unwrap();
    assert!(load_runtime_media_provider_bridge_config(&symlink_data).is_err());

    let race_path = temp.path().join("race-config");
    fs::write(&race_path, b"preserved-race-winner").unwrap();
    assert!(super::create_private_file_atomically(&race_path, b"loser", 0o600, "config").is_err());
    assert_eq!(fs::read(race_path).unwrap(), b"preserved-race-winner");
}

#[cfg(unix)]
#[test]
fn media_prerequisite_conflict_cleans_only_files_created_by_that_attempt() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let source = temp.path().join("source");
    owner_only_dir(&source);
    write_media_prerequisite(&source.join("ffmpeg"), b"ffmpeg-source", 0o700);
    write_media_prerequisite(&source.join("ffprobe"), b"ffprobe-source", 0o700);
    let tools = data_dir.join("protected-content/media-provider/tools");
    owner_only_dir(&data_dir);
    owner_only_dir(&data_dir.join("protected-content"));
    owner_only_dir(&data_dir.join("protected-content/media-provider"));
    owner_only_dir(&tools);
    write_media_prerequisite(&tools.join("ffprobe"), b"preserved-conflict", 0o500);

    let error =
        prepare_runtime_media_provider_prerequisite_with_path(&data_dir, source.as_os_str())
            .unwrap_err();

    assert!(error.to_string().contains("ffprobe import conflicts"));
    assert_eq!(
        fs::read(tools.join("ffprobe")).unwrap(),
        b"preserved-conflict"
    );
    assert!(!tools.join("ffmpeg").exists());
    assert!(!media_config_path(&data_dir).exists());
}

#[cfg(unix)]
fn test_chain_provider_config() -> Value {
    json!({
        "schema": PROTECTED_CONTENT_CHAIN_PROVIDER_CONFIG_SCHEMA_V1,
        "protected_content_network": {
            "id": "esc-mainnet",
            "rpc_url": "https://private-primary.example.invalid"
        }
    })
}

#[cfg(unix)]
fn canonical_b64<T: CanonicalContract>(value: &T) -> String {
    base64::engine::general_purpose::STANDARD.encode(value.canonical_bytes().unwrap())
}

#[cfg(unix)]
fn raw_b64_32(bytes: [u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(unix)]
fn peer_did_for_seed(seed: u8) -> String {
    crate::crypto::encode_signing_key_did(&SigningKey::from_bytes(&[seed; 32]))
}

#[cfg(unix)]
fn custody_composition_config(
    now: u64,
    routes: Vec<RuntimeCustodyRouteBindingConfig>,
) -> RuntimeCustodyCompositionConfigFile {
    let epoch = signed_custody_epoch();
    let pool = signed_custody_pool_for_epoch(&epoch, (now.saturating_sub(60), now + 3600));
    let authorization =
        signed_committee_authorization_for_epoch(pool.pool_identity().unwrap(), &epoch);
    RuntimeCustodyCompositionConfigFile {
        schema: CUSTODY_COMPOSITION_SCHEMA_V1.to_string(),
        expected_policy_authority_base64: raw_b64_32(
            SigningKey::from_bytes(&[0x71; 32])
                .verifying_key()
                .to_bytes(),
        ),
        expected_committee_authorization_identity_base64: canonical_b64(
            &authorization.authorization_identity().unwrap(),
        ),
        signed_pool_base64: canonical_b64(&pool),
        signed_epoch_base64: canonical_b64(&epoch),
        signed_committee_authorization_base64: canonical_b64(&authorization),
        routes,
    }
}

#[cfg(unix)]
fn custody_route_bindings(
    epoch: &SignedCustodyEpochV1,
    transports: [RuntimeCustodyRouteTransportConfig; 3],
) -> Vec<RuntimeCustodyRouteBindingConfig> {
    epoch
        .statement()
        .nodes()
        .iter()
        .zip(transports)
        .enumerate()
        .map(
            |(index, (node, transport))| RuntimeCustodyRouteBindingConfig {
                node_public_key_base64: raw_b64_32(*node.node_public_key().as_bytes()),
                owner_state_root_base64: raw_b64_32([0x40 + u8::try_from(index).unwrap(); 32]),
                transport,
            },
        )
        .collect()
}

#[cfg(unix)]
fn write_owner_only_custody_composition_config(
    data_dir: &Path,
    config: &RuntimeCustodyCompositionConfigFile,
) {
    let root = protected_content_root(data_dir);
    owner_only_dir(&root);
    let path = custody_composition_config_path(data_dir);
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::to_value(config).unwrap()).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
fn assert_runtime_custody_transports(
    composition: &RuntimeCustodyComposition,
    expected: &[ProviderInvocationTransport],
) {
    assert_eq!(composition.nodes.len(), expected.len());
    for (node, expected_transport) in composition.nodes.iter().zip(expected.iter()) {
        assert_eq!(&node.adapter.transport, expected_transport);
    }
    for window in composition.nodes.windows(2) {
        assert!(Arc::ptr_eq(
            &window[0].adapter.registry,
            &window[1].adapter.registry
        ));
    }
}

#[cfg(unix)]
fn mint_draft_for_composition_journal_test() -> RuntimeMintDraft {
    let epoch = signed_custody_epoch();
    let envelope = custody_envelope_for_media_with_epoch(0x41, &epoch);
    let (init_segment, encrypted_segments) = media_components(0x41);
    let mint_nodes = epoch
        .statement()
        .nodes()
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let node_seed = u8::try_from(index + 1).unwrap();
            RuntimeMintNodeBinding::new(
                node.node_public_key(),
                CustodyPoolOperatorIdV1::new([0x80 + node_seed; 32]),
                CustodyPoolFailureDomainIdV1::new([0x90 + node_seed; 32]),
                Digest32::new([0x3f + node_seed; 32]),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    RuntimeMintDraft::new(
        &init_segment,
        &encrypted_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
        content_access_id(0x41),
        envelope.key_envelope_identity().unwrap(),
        policy_body().policy_identity().unwrap(),
        envelope.manifest().content_key_commitment(),
        envelope.manifest().threshold(),
        mint_nodes,
    )
    .unwrap()
}

#[cfg(unix)]
fn inactive_custody_state_root(data_dir: &Path) -> PathBuf {
    data_dir.join("protected-content/custody-provider/inactive")
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
enum ProtectedStartupProvider {
    Protect,
    Custody,
    Decrypt,
}

#[cfg(unix)]
impl ProtectedStartupProvider {
    const ALL: [Self; 3] = [Self::Protect, Self::Custody, Self::Decrypt];

    fn name(self) -> &'static str {
        match self {
            Self::Protect => "protect-provider",
            Self::Custody => "custody-provider",
            Self::Decrypt => "protected-content-decrypt-provider",
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Protect => PROTECT_PROVIDER_ID,
            Self::Custody => CUSTODY_PROVIDER_ID,
            Self::Decrypt => PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
        }
    }

    fn status(self) -> Value {
        match self {
            Self::Protect => json!({
                "status": "ok",
                "data": {
                    "provider": PROTECT_PROVIDER_PROCESS_ID,
                    "version": PROTECT_PROVIDER_VERSION,
                    "configured": true,
                    "supported_operations": PROTECT_PROVIDER_OPERATIONS,
                    "request_schema": PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
                    "response_schema": PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
                }
            }),
            Self::Custody => json!({
                "status": "ok",
                "data": {
                    "provider": CUSTODY_PROVIDER_ID,
                    "version": CUSTODY_PROVIDER_VERSION,
                    "configured": true,
                    "supported_operations": CUSTODY_PROVIDER_OPERATIONS,
                    "request_schema": CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
                    "response_schema": CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
                }
            }),
            Self::Decrypt => protected_content_decrypt_provider_status(),
        }
    }

    fn prepare_private_config(self, data_dir: &Path) {
        owner_only_dir(data_dir);
        if matches!(self, Self::Custody) {
            owner_only_dir(&inactive_custody_state_root(data_dir));
        }
    }

    async fn register(
        self,
        registry: &Arc<ProviderRegistry>,
        binary: &Path,
        data_dir: &Path,
    ) -> anyhow::Result<()> {
        match self {
            Self::Protect => register_protect_provider(registry, binary).await,
            Self::Custody => register_inactive_custody_provider(registry, binary, data_dir).await,
            Self::Decrypt => {
                register_protected_content_decrypt_provider(
                    registry,
                    binary,
                    runtime_operation_issuer_for_seed(0x41),
                )
                .await
            }
        }
    }
}

#[cfg(unix)]
fn write_mock_custody_provider(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let binary = root.join("mock-custody-provider.sh");
    let request_log = root.join("mock-custody-provider.requests");
    let pid_file = root.join("mock-custody-provider.pid");
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" >> '{}'\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in\n    *'\"op\":\"shutdown\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}'; exit 0 ;;\n    *'\"unexpected\"'* ) printf '%s\\n' '{{\"status\":\"error\",\"code\":\"invalid_request\"}}' ;;\n    *'\"op\":\"status\"'*) printf '%s\\n' '{}' ;;\n    *'\"op\":\"release_contribution\"'*) printf '%s\\n' '{{\"status\":\"ok\",\"data\":{{\"echo\":\"custody\"}}}}' ;;\n    *) printf '%s\\n' '{{\"status\":\"ok\",\"data\":{{\"echo\":\"init\"}}}}' ;;\n  esac\ndone\n",
        pid_file.display(),
        request_log.display(),
        ProtectedStartupProvider::Custody.status(),
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (binary, pid_file, request_log)
}

#[cfg(unix)]
fn write_mock_protected_startup_provider(
    root: &Path,
    provider_name: &str,
    status_response: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    write_mock_protected_startup_provider_with_shutdown(
        root,
        provider_name,
        status_response,
        r#"{"status":"ok"}"#,
    )
}

#[cfg(unix)]
fn write_mock_protected_startup_provider_with_shutdown(
    root: &Path,
    provider_name: &str,
    status_response: &str,
    shutdown_response: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    let binary = root.join(format!("mock-{provider_name}.sh"));
    let request_log = root.join(format!("mock-{provider_name}.requests"));
    let pid_file = root.join(format!("mock-{provider_name}.pid"));
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" >> '{}'\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in\n    *'\"op\":\"shutdown\"'*) printf '%s\\n' '{}'; exit 0 ;;\n    *'\"op\":\"status\"'*) printf '%s\\n' '{}' ;;\n    *'\"op\":\"init\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}' ;;\n    *) printf '%s\\n' '{{\"status\":\"error\",\"code\":\"unexpected\"}}' ;;\n  esac\ndone\n",
        pid_file.display(),
        request_log.display(),
        shutdown_response,
        status_response,
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (binary, pid_file, request_log)
}

#[cfg(unix)]
fn create_test_fifo(path: &Path) {
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
}

#[cfg(unix)]
fn write_blocking_protected_status_provider(
    root: &Path,
    provider: ProtectedStartupProvider,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let binary = root.join(format!("mock-{}-timeout.sh", provider.name()));
    let request_log = root.join(format!("mock-{}-timeout.requests", provider.name()));
    let pid_file = root.join(format!("mock-{}-timeout.pid", provider.name()));
    let status_signal = root.join(format!("mock-{}-status.signal", provider.name()));
    let status_release = root.join(format!("mock-{}-status.release", provider.name()));
    create_test_fifo(&status_signal);
    create_test_fifo(&status_release);
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" >> '{}'\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in\n    *'\"op\":\"shutdown\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}'; exit 0 ;;\n    *'\"op\":\"status\"'*) printf x > '{}'; cat '{}' >/dev/null; printf '%s\\n' '{}' ;;\n    *'\"op\":\"init\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}' ;;\n    *) printf '%s\\n' '{{\"status\":\"error\",\"code\":\"unexpected\"}}' ;;\n  esac\ndone\n",
        pid_file.display(),
        request_log.display(),
        status_signal.display(),
        status_release.display(),
        provider.status(),
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (binary, pid_file, request_log, status_signal, status_release)
}

#[cfg(unix)]
fn status_with_field(provider: ProtectedStartupProvider, field: &str, value: Value) -> Value {
    let mut status = provider.status();
    status["data"][field] = value;
    status
}

#[cfg(unix)]
fn protected_startup_status_mismatches(
    provider: ProtectedStartupProvider,
) -> Vec<(&'static str, Value)> {
    let mut extra_operations = provider.status()["data"]["supported_operations"]
        .as_array()
        .unwrap()
        .clone();
    extra_operations.push(json!("unexpected"));
    let mut reordered_operations = provider.status()["data"]["supported_operations"]
        .as_array()
        .unwrap()
        .clone();
    reordered_operations.swap(0, 1);
    let mut extra_data = provider.status();
    extra_data["data"]["route"] = json!("private");
    let mut extra_top = provider.status();
    extra_top["route"] = json!("private");
    vec![
        (
            "identity",
            status_with_field(provider, "provider", json!("wrong-provider")),
        ),
        (
            "version",
            status_with_field(provider, "version", json!("wrong-version")),
        ),
        (
            "configured",
            status_with_field(provider, "configured", json!(false)),
        ),
        (
            "request-schema",
            status_with_field(provider, "request_schema", json!("wrong-request-schema")),
        ),
        (
            "response-schema",
            status_with_field(provider, "response_schema", json!("wrong-response-schema")),
        ),
        (
            "operations-missing",
            status_with_field(provider, "supported_operations", json!(["status"])),
        ),
        (
            "operations-extra",
            status_with_field(
                provider,
                "supported_operations",
                Value::Array(extra_operations),
            ),
        ),
        (
            "operations-reordered",
            status_with_field(
                provider,
                "supported_operations",
                Value::Array(reordered_operations),
            ),
        ),
        ("extra-data", extra_data),
        ("extra-top", extra_top),
    ]
}

#[cfg(unix)]
fn protected_content_decrypt_provider_status() -> Value {
    json!({
        "status": "ok",
        "data": {
            "provider": PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            "version": PROTECTED_CONTENT_DECRYPT_PROVIDER_VERSION,
            "configured": true,
            "supported_operations": PROTECTED_CONTENT_DECRYPT_PROVIDER_OPERATIONS,
            "request_schema": DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
            "response_schema": DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
        }
    })
}

#[cfg(unix)]
fn write_mock_protected_content_decrypt_provider(
    root: &Path,
    expected_issuer: RuntimeOperationIssuerKeyV1,
    status: &Value,
) -> (PathBuf, PathBuf, PathBuf) {
    let binary = root.join("mock-protected-content-decrypt-provider.sh");
    let request_log = root.join("mock-protected-content-decrypt-provider.requests");
    let pid_file = root.join("mock-protected-content-decrypt-provider.pid");
    let expected_issuer = format!("0x{}", hex::encode(expected_issuer.as_bytes()));
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" >> '{}'\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in\n    *'\"op\":\"shutdown\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}'; exit 0 ;;\n    *'\"op\":\"init\"'*)\n      case \"$line\" in\n        *'\"trusted_runtime_issuer\":\"{}\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}' ;;\n        *) printf '%s\\n' '{{\"status\":\"error\",\"code\":\"invalid_config\",\"message\":\"wrong issuer\"}}' ;;\n      esac ;;\n    *'\"op\":\"status\"'*) printf '%s\\n' '{}' ;;\n    *) printf '%s\\n' '{{\"status\":\"error\",\"code\":\"unexpected\",\"message\":\"unexpected\"}}' ;;\n  esac\ndone\n",
        pid_file.display(),
        request_log.display(),
        expected_issuer,
        status,
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    (binary, pid_file, request_log)
}

#[cfg(unix)]
fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn protected_provider_startup_rejects_status_mismatches_and_reaps_child() {
    for provider in ProtectedStartupProvider::ALL {
        for (case, status) in protected_startup_status_mismatches(provider) {
            let temp = tempfile::tempdir().unwrap();
            let case_root = temp.path().join(case);
            fs::create_dir_all(&case_root).unwrap();
            let data_dir = case_root.join("data");
            provider.prepare_private_config(&data_dir);
            let (binary, pid_file, request_log) = write_mock_protected_startup_provider(
                &case_root,
                provider.name(),
                &status.to_string(),
            );
            let registry = Arc::new(ProviderRegistry::new());

            provider
                .register(&registry, &binary, &data_dir)
                .await
                .expect_err("mismatched provider status must fail closed");

            assert!(
                !process_is_running(read_pid(&pid_file)),
                "{provider:?} {case}"
            );
            let requests = fs::read_to_string(request_log).unwrap();
            assert!(requests.contains(r#""op":"status""#), "{provider:?} {case}");
            assert!(
                requests.contains(r#""op":"shutdown""#),
                "{provider:?} {case}"
            );
            assert!(
                !registry
                    .has_ready_runtime_provider_target(provider.target())
                    .await
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn protected_provider_startup_rejects_invalid_status_and_reaps_child() {
    for provider in ProtectedStartupProvider::ALL {
        for (case, status) in [
            ("malformed-json", "not-json"),
            ("missing-data", r#"{"status":"ok"}"#),
            (
                "error",
                r#"{"status":"error","code":"not_ready","message":"not ready"}"#,
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let case_root = temp.path().join(case);
            fs::create_dir_all(&case_root).unwrap();
            let data_dir = case_root.join("data");
            provider.prepare_private_config(&data_dir);
            let (binary, pid_file, request_log) =
                write_mock_protected_startup_provider(&case_root, provider.name(), status);
            let registry = Arc::new(ProviderRegistry::new());

            provider
                .register(&registry, &binary, &data_dir)
                .await
                .expect_err("invalid provider status must fail closed");

            assert!(
                !process_is_running(read_pid(&pid_file)),
                "{provider:?} {case}"
            );
            let requests = fs::read_to_string(request_log).unwrap();
            assert!(requests.contains(r#""op":"status""#), "{provider:?} {case}");
            assert!(
                requests.contains(r#""op":"shutdown""#),
                "{provider:?} {case}"
            );
            assert!(
                !registry
                    .has_ready_runtime_provider_target(provider.target())
                    .await
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn protected_provider_startup_timeout_sends_shutdown_and_reaps_child() {
    for provider in ProtectedStartupProvider::ALL {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        provider.prepare_private_config(&data_dir);
        let (binary, pid_file, request_log, status_signal, status_release) =
            write_blocking_protected_status_provider(temp.path(), provider);
        let registry = Arc::new(ProviderRegistry::new());
        let task_registry = registry.clone();
        let task_data_dir = data_dir.clone();
        let registration = tokio::spawn(async move {
            provider
                .register(&task_registry, &binary, &task_data_dir)
                .await
        });

        let mut signal = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&status_signal)
            .await
            .unwrap();
        let mut marker = [0u8; 1];
        signal.read_exact(&mut marker).await.unwrap();
        assert_eq!(marker, [b'x']);
        tokio::time::pause();
        let release_status = tokio::spawn(async move {
            let release = tokio::time::timeout(
                PROTECTED_CONTENT_PROVIDER_STATUS_TIMEOUT + std::time::Duration::from_millis(1),
                std::future::pending::<()>(),
            )
            .await;
            assert!(release.is_err());
            let mut release = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&status_release)
                .await
                .unwrap();
            release.write_all(b"x").await.unwrap();
        });
        tokio::task::yield_now().await;
        tokio::time::advance(
            PROTECTED_CONTENT_PROVIDER_STATUS_TIMEOUT + std::time::Duration::from_millis(1),
        )
        .await;
        tokio::time::resume();
        release_status.await.unwrap();

        let error = registration.await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("status timed out"),
            "{provider:?}: {error}"
        );
        let requests = fs::read_to_string(request_log).unwrap();
        assert!(
            !process_is_running(read_pid(&pid_file)),
            "{provider:?}: {error}; requests={requests}"
        );
        assert!(requests.contains(r#""op":"status""#), "{provider:?}");
        assert!(requests.contains(r#""op":"shutdown""#), "{provider:?}");
        assert!(
            !registry
                .has_ready_runtime_provider_target(provider.target())
                .await
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn protected_provider_duplicate_unregister_and_restart_settle_exact_processes() {
    for provider in ProtectedStartupProvider::ALL {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = temp.path().join("data");
        provider.prepare_private_config(&data_dir);
        let (binary, pid_file, request_log) = write_mock_protected_startup_provider(
            temp.path(),
            provider.name(),
            &provider.status().to_string(),
        );
        let registry = Arc::new(ProviderRegistry::new());

        provider
            .register(&registry, &binary, &data_dir)
            .await
            .unwrap();
        let first_pid = read_pid(&pid_file);
        assert!(process_is_running(first_pid), "{provider:?}");
        assert!(
            registry
                .has_ready_runtime_provider_target(provider.target())
                .await
        );

        let duplicate = provider
            .register(&registry, &binary, &data_dir)
            .await
            .unwrap_err();
        assert!(duplicate.to_string().contains("already registered"));
        let pids = read_pids(&pid_file);
        assert_eq!(pids.len(), 2, "{provider:?}");
        let rejected_pid = pids[1];
        assert!(process_is_running(first_pid), "{provider:?}");
        assert!(!process_is_running(rejected_pid), "{provider:?}");

        registry
            .unregister_runtime_provider_target(provider.target())
            .await
            .unwrap();
        assert!(!process_is_running(first_pid), "{provider:?}");

        provider
            .register(&registry, &binary, &data_dir)
            .await
            .unwrap();
        let replacement_pid = read_pid(&pid_file);
        assert_ne!(replacement_pid, first_pid);
        assert_ne!(replacement_pid, rejected_pid);
        assert!(process_is_running(replacement_pid), "{provider:?}");
        registry
            .unregister_runtime_provider_target(provider.target())
            .await
            .unwrap();
        assert!(!process_is_running(replacement_pid), "{provider:?}");
        assert_eq!(
            fs::read_to_string(request_log)
                .unwrap()
                .matches(r#""op":"shutdown""#)
                .count(),
            3,
            "{provider:?}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn protected_provider_startup_keeps_primary_error_with_shutdown_failure_context() {
    let temp = tempfile::tempdir().unwrap();
    let provider = ProtectedStartupProvider::Protect;
    let data_dir = temp.path().join("data");
    provider.prepare_private_config(&data_dir);
    let mut status = provider.status();
    status["data"]["provider"] = json!("wrong-provider");
    let (binary, pid_file, request_log) = write_mock_protected_startup_provider_with_shutdown(
        temp.path(),
        provider.name(),
        &status.to_string(),
        r#"{"status":"error","code":"shutdown_failed","message":"denied"}"#,
    );
    let registry = Arc::new(ProviderRegistry::new());

    let error = provider
        .register(&registry, &binary, &data_dir)
        .await
        .unwrap_err()
        .to_string();

    let primary = error.find("unsupported identity").unwrap();
    let cleanup = error.find("shutdown/reap also failed").unwrap();
    assert!(primary < cleanup, "{error}");
    assert!(!process_is_running(read_pid(&pid_file)));
    assert!(fs::read_to_string(request_log)
        .unwrap()
        .contains(r#""op":"shutdown""#));
}

#[cfg(unix)]
#[test]
fn runtime_custody_composition_loads_valid_owner_only_config_with_local_and_carrier_routes() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let peer_did_1 = peer_did_for_seed(0xa1);
    let peer_did_2 = peer_did_for_seed(0xa2);
    let config = custody_composition_config(
        now,
        custody_route_bindings(
            &epoch,
            [
                RuntimeCustodyRouteTransportConfig::Local,
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_1.clone(),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_2.clone(),
                },
            ],
        ),
    );
    write_owner_only_custody_composition_config(&data_dir, &config);

    let config_text =
        String::from_utf8(fs::read(custody_composition_config_path(&data_dir)).unwrap()).unwrap();
    assert!(config_text.contains(&peer_did_1));
    assert!(config_text.contains(&peer_did_2));
    assert!(config_text.contains(&config.signed_pool_base64));

    let loaded = load_runtime_custody_composition_config(&data_dir)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.expected_policy_authority,
        signed_custody_epoch().statement().issuer()
    );
    assert_eq!(
        loaded.signed_pool.canonical_bytes().unwrap(),
        base64::engine::general_purpose::STANDARD
            .decode(&config.signed_pool_base64)
            .unwrap()
    );
    assert!(matches!(
        loaded.routes[0].transport,
        ProviderInvocationTransport::Local
    ));
    assert!(matches!(
        &loaded.routes[1].transport,
        ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
            peer_did,
            timeout_ms: None,
        }) if peer_did == &peer_did_1
    ));
    assert!(matches!(
        &loaded.routes[2].transport,
        ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
            peer_did,
            timeout_ms: None,
        }) if peer_did == &peer_did_2
    ));

    let registry = Arc::new(ProviderRegistry::new());
    let composition = load_runtime_custody_composition(&data_dir, registry)
        .unwrap()
        .unwrap();
    assert_runtime_custody_transports(
        &composition,
        &[
            ProviderInvocationTransport::Local,
            ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: peer_did_1.clone(),
                timeout_ms: None,
            }),
            ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: peer_did_2.clone(),
                timeout_ms: None,
            }),
        ],
    );
    let configured = composition.configured_nodes().unwrap();
    assert_eq!(
        resolve_runtime_mint_selected_nodes(
            composition.expected_policy_authority,
            composition.expected_authorization_identity,
            &composition.signed_pool,
            &composition.signed_epoch,
            &composition.signed_committee_authorization,
            now,
            &configured,
        )
        .unwrap()
        .len(),
        3
    );
    assert!(!std::ptr::eq(
        &composition.nodes[0].adapter,
        &composition.nodes[1].adapter
    ));

    let debug = format!("{composition:?}");
    assert!(!debug.contains(&peer_did_1));
    assert!(!debug.contains(&peer_did_2));
    assert!(!debug.contains(&config.signed_pool_base64));
    assert!(!debug.contains(&config.signed_epoch_base64));
    assert!(!debug.contains(&config.signed_committee_authorization_base64));
}

#[cfg(unix)]
#[test]
fn runtime_custody_composition_accepts_three_remote_routes() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let peers = [
        peer_did_for_seed(0xb1),
        peer_did_for_seed(0xb2),
        peer_did_for_seed(0xb3),
    ];
    let config = custody_composition_config(
        now,
        custody_route_bindings(
            &epoch,
            [
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peers[0].clone(),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peers[1].clone(),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peers[2].clone(),
                },
            ],
        ),
    );
    write_owner_only_custody_composition_config(&data_dir, &config);

    let loaded = load_runtime_custody_composition_config(&data_dir)
        .unwrap()
        .unwrap();
    for (route, peer_did) in loaded.routes.iter().zip(peers.iter()) {
        assert!(matches!(
            &route.transport,
            ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: actual,
                timeout_ms: None,
            }) if actual == peer_did
        ));
    }

    let composition =
        load_runtime_custody_composition(&data_dir, Arc::new(ProviderRegistry::new()))
            .unwrap()
            .unwrap();
    assert_runtime_custody_transports(
        &composition,
        &[
            ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: peers[0].clone(),
                timeout_ms: None,
            }),
            ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: peers[1].clone(),
                timeout_ms: None,
            }),
            ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                peer_did: peers[2].clone(),
                timeout_ms: None,
            }),
        ],
    );
}

#[cfg(unix)]
#[test]
fn runtime_custody_composition_absent_returns_none_without_creating_state() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);

    let config = load_runtime_custody_composition_config(&data_dir).unwrap();
    assert!(config.is_none());
    assert!(!protected_content_root(&data_dir).exists());
}

#[cfg(unix)]
#[test]
fn runtime_custody_composition_rejects_trust_anchor_and_route_mismatches() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let peer_did_1 = peer_did_for_seed(0xc1);
    let peer_did_2 = peer_did_for_seed(0xc2);
    let base_routes = custody_route_bindings(
        &epoch,
        [
            RuntimeCustodyRouteTransportConfig::Local,
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: peer_did_1.clone(),
            },
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: peer_did_2.clone(),
            },
        ],
    );

    let mut wrong_policy = custody_composition_config(now, base_routes.clone());
    wrong_policy.expected_policy_authority_base64 = raw_b64_32(
        SigningKey::from_bytes(&[0x72; 32])
            .verifying_key()
            .to_bytes(),
    );
    write_owner_only_custody_composition_config(&data_dir, &wrong_policy);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected trust-anchor rejection")
        .to_string()
        .contains("trust anchors"));

    let mut wrong_auth = custody_composition_config(now, base_routes.clone());
    wrong_auth.expected_committee_authorization_identity_base64 =
        canonical_b64(&CustodyCommitteeAuthorizationIdentityV1::new(digest(0xfe), 1).unwrap());
    write_owner_only_custody_composition_config(&data_dir, &wrong_auth);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected authorization-identity rejection")
        .to_string()
        .contains("trust anchors"));

    let mut missing_route = custody_composition_config(now, base_routes.clone());
    missing_route.routes.pop();
    write_owner_only_custody_composition_config(&data_dir, &missing_route);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected missing-route rejection")
        .to_string()
        .contains("exactly three selected nodes"));

    let mut extra_route = custody_composition_config(now, base_routes.clone());
    extra_route.routes.push(RuntimeCustodyRouteBindingConfig {
        node_public_key_base64: raw_b64_32(*node_public_key(9).as_bytes()),
        owner_state_root_base64: raw_b64_32([0x99; 32]),
        transport: RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
            peer_did: peer_did_for_seed(0xc9),
        },
    });
    write_owner_only_custody_composition_config(&data_dir, &extra_route);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected extra-route rejection")
        .to_string()
        .contains("exactly three selected nodes"));

    let mut duplicate_route = custody_composition_config(now, base_routes.clone());
    duplicate_route.routes[1].node_public_key_base64 =
        duplicate_route.routes[0].node_public_key_base64.clone();
    write_owner_only_custody_composition_config(&data_dir, &duplicate_route);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected duplicate-node rejection")
        .to_string()
        .contains("duplicated or invalid"));

    let mut duplicate_root = custody_composition_config(now, base_routes.clone());
    duplicate_root.routes[1].owner_state_root_base64 =
        duplicate_root.routes[0].owner_state_root_base64.clone();
    write_owner_only_custody_composition_config(&data_dir, &duplicate_root);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected duplicate-root rejection")
        .to_string()
        .contains("duplicated or invalid"));

    let mut two_local = custody_composition_config(now, base_routes.clone());
    two_local.routes[1].transport = RuntimeCustodyRouteTransportConfig::Local;
    write_owner_only_custody_composition_config(&data_dir, &two_local);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected multi-local rejection")
        .to_string()
        .contains("at most one local"));

    let mut duplicate_peer = custody_composition_config(now, base_routes);
    duplicate_peer.routes[2].transport = RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
        peer_did: peer_did_1,
    };
    write_owner_only_custody_composition_config(&data_dir, &duplicate_peer);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected duplicate-peer rejection")
        .to_string()
        .contains("carrier peer DIDs must be distinct"));

    let mut foreign_node = custody_composition_config(
        now,
        custody_route_bindings(
            &epoch,
            [
                RuntimeCustodyRouteTransportConfig::Local,
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xca),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xcb),
                },
            ],
        ),
    );
    foreign_node.routes[2].node_public_key_base64 = raw_b64_32(*node_public_key(9).as_bytes());
    write_owner_only_custody_composition_config(&data_dir, &foreign_node);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected foreign-node rejection")
        .to_string()
        .contains("signed node set"));
}

#[cfg(unix)]
#[test]
fn runtime_custody_composition_rejects_noncanonical_json_and_base64_and_bad_peer_did() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let mut config = custody_composition_config(
        now,
        custody_route_bindings(
            &epoch,
            [
                RuntimeCustodyRouteTransportConfig::Local,
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xdb),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xdc),
                },
            ],
        ),
    );
    owner_only_dir(&protected_content_root(&data_dir));
    let path = custody_composition_config_path(&data_dir);

    fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected noncanonical-json rejection")
        .to_string()
        .contains("not canonical"));

    write_owner_only_custody_composition_config(&data_dir, &config);
    config.signed_pool_base64.push('\n');
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::to_value(&config).unwrap()).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected base64 rejection")
        .to_string()
        .contains("base64"));

    let mut bad_peer = custody_composition_config(
        now,
        custody_route_bindings(
            &epoch,
            [
                RuntimeCustodyRouteTransportConfig::Local,
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: "not-a-did".to_string(),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xdd),
                },
            ],
        ),
    );
    write_owner_only_custody_composition_config(&data_dir, &bad_peer);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected invalid did:key rejection")
        .to_string()
        .contains("did:key"));

    bad_peer.routes[1].transport = RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
        peer_did: format!("{} ", peer_did_for_seed(0xde)),
    };
    write_owner_only_custody_composition_config(&data_dir, &bad_peer);
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected noncanonical did:key rejection")
        .to_string()
        .contains("did:key"));
}

#[cfg(unix)]
#[test]
fn protected_content_chain_config_loads_owner_only_file_and_accepts_missing_state() {
    let temp = tempfile::tempdir().unwrap();
    let missing_data_dir = temp.path().join("missing");
    owner_only_dir(&missing_data_dir);
    assert!(
        load_runtime_protected_content_chain_provider_config(&missing_data_dir)
            .unwrap()
            .is_none()
    );

    let data_dir = temp.path().join("configured");
    owner_only_dir(&data_dir);
    let config = test_chain_provider_config();
    write_chain_provider_config(&data_dir, &config);

    let loaded = load_runtime_protected_content_chain_provider_config(&data_dir)
        .unwrap()
        .expect("owner-only Chain config");
    assert_eq!(
        loaded.protected_content_network(),
        &config["protected_content_network"]
    );
    assert!(config.get("protected_content_runtime_issuer").is_none());
}

#[cfg(unix)]
#[test]
fn protected_content_chain_config_rejects_unknown_empty_oversized_or_invalid_content() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);

    let mut unknown = test_chain_provider_config();
    unknown["protected_content_runtime_issuer"] = json!("0xoperator-cannot-set-this");
    write_chain_provider_config(&data_dir, &unknown);
    assert!(load_runtime_protected_content_chain_provider_config(&data_dir).is_err());

    for invalid in [
        Vec::new(),
        b"{".to_vec(),
        serde_json::to_vec(&json!({
            "schema": "wrong-schema",
            "protected_content_network": {}
        }))
        .unwrap(),
        serde_json::to_vec(&json!({
            "schema": PROTECTED_CONTENT_CHAIN_PROVIDER_CONFIG_SCHEMA_V1,
            "protected_content_network": []
        }))
        .unwrap(),
        vec![b'x'; MAX_CHAIN_PROVIDER_CONFIG_BYTES + 1],
    ] {
        fs::write(chain_provider_config_path(&data_dir), invalid).unwrap();
        fs::set_permissions(
            chain_provider_config_path(&data_dir),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(load_runtime_protected_content_chain_provider_config(&data_dir).is_err());
    }
}

#[cfg(unix)]
#[test]
fn protected_content_chain_config_rejects_unsafe_or_nonregular_paths() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_chain_provider_config();

    let mode_data_dir = temp.path().join("mode");
    owner_only_dir(&mode_data_dir);
    write_chain_provider_config(&mode_data_dir, &config);
    fs::set_permissions(
        chain_provider_config_path(&mode_data_dir),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    assert!(load_runtime_protected_content_chain_provider_config(&mode_data_dir).is_err());

    fs::set_permissions(
        chain_provider_config_path(&mode_data_dir),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    fs::set_permissions(
        protected_content_root(&mode_data_dir),
        fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    assert!(load_runtime_protected_content_chain_provider_config(&mode_data_dir).is_err());

    let directory_data_dir = temp.path().join("directory");
    owner_only_dir(&directory_data_dir);
    owner_only_dir(&protected_content_root(&directory_data_dir));
    owner_only_dir(&chain_provider_config_path(&directory_data_dir));
    assert!(load_runtime_protected_content_chain_provider_config(&directory_data_dir).is_err());

    let symlink_data_dir = temp.path().join("symlink");
    owner_only_dir(&symlink_data_dir);
    owner_only_dir(&protected_content_root(&symlink_data_dir));
    let symlink_target = temp.path().join("chain-provider-target.json");
    fs::write(&symlink_target, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&symlink_target, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(
        &symlink_target,
        chain_provider_config_path(&symlink_data_dir),
    )
    .unwrap();
    assert!(load_runtime_protected_content_chain_provider_config(&symlink_data_dir).is_err());

    let symlink_parent_data_dir = temp.path().join("symlink-parent");
    owner_only_dir(&symlink_parent_data_dir);
    let symlink_parent_target = temp.path().join("protected-content-target");
    owner_only_dir(&symlink_parent_target);
    let symlink_parent_config = symlink_parent_target.join("chain-provider.json");
    fs::write(&symlink_parent_config, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&symlink_parent_config, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(
        &symlink_parent_target,
        protected_content_root(&symlink_parent_data_dir),
    )
    .unwrap();
    assert!(
        load_runtime_protected_content_chain_provider_config(&symlink_parent_data_dir).is_err()
    );

    let hard_link_data_dir = temp.path().join("hard-link");
    owner_only_dir(&hard_link_data_dir);
    owner_only_dir(&protected_content_root(&hard_link_data_dir));
    let hard_link_source = temp.path().join("chain-provider-source.json");
    fs::write(&hard_link_source, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&hard_link_source, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(
        &hard_link_source,
        chain_provider_config_path(&hard_link_data_dir),
    )
    .unwrap();
    assert!(load_runtime_protected_content_chain_provider_config(&hard_link_data_dir).is_err());
}

#[cfg(unix)]
#[test]
fn runtime_custody_composition_rejects_unsafe_or_symlinked_paths() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let config = custody_composition_config(
        now,
        custody_route_bindings(
            &epoch,
            [
                RuntimeCustodyRouteTransportConfig::Local,
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xd1),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xd2),
                },
            ],
        ),
    );

    write_owner_only_custody_composition_config(&data_dir, &config);
    fs::set_permissions(
        custody_composition_config_path(&data_dir),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected unsafe-file-mode rejection")
        .to_string()
        .contains("owner-only"));

    write_owner_only_custody_composition_config(&data_dir, &config);
    fs::set_permissions(
        protected_content_root(&data_dir),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(load_runtime_custody_composition_config(&data_dir)
        .err()
        .expect("expected unsafe-parent-mode rejection")
        .to_string()
        .contains("owner-only"));

    write_owner_only_custody_composition_config(&data_dir, &config);
    let symlink_data_dir = temp.path().join("symlink-data");
    owner_only_dir(&symlink_data_dir);
    let symlink_root_target = symlink_data_dir.join("protected-root-target");
    owner_only_dir(&symlink_root_target);
    std::os::unix::fs::symlink(
        &symlink_root_target,
        protected_content_root(&symlink_data_dir),
    )
    .unwrap();
    let symlink_config_path = symlink_root_target.join("custody-composition.json");
    fs::write(
        &symlink_config_path,
        serde_json::to_vec(&serde_json::to_value(&config).unwrap()).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&symlink_config_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(load_runtime_custody_composition_config(&symlink_data_dir)
        .err()
        .expect("expected symlink-parent rejection")
        .to_string()
        .contains("owner-only directory"));

    let symlink_file_data_dir = temp.path().join("symlink-file-data");
    owner_only_dir(&symlink_file_data_dir);
    owner_only_dir(&protected_content_root(&symlink_file_data_dir));
    let real_config_dir = temp.path().join("real-config");
    owner_only_dir(&real_config_dir);
    let real_config_path = real_config_dir.join("custody-composition.json");
    fs::write(
        &real_config_path,
        serde_json::to_vec(&serde_json::to_value(&config).unwrap()).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&real_config_path, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(
        &real_config_path,
        custody_composition_config_path(&symlink_file_data_dir),
    )
    .unwrap();
    assert!(
        load_runtime_custody_composition_config(&symlink_file_data_dir)
            .err()
            .expect("expected symlink-file rejection")
            .to_string()
            .contains("unavailable")
    );

    let directory_path_data_dir = temp.path().join("directory-path-data");
    owner_only_dir(&directory_path_data_dir);
    owner_only_dir(&protected_content_root(&directory_path_data_dir));
    std::fs::create_dir(custody_composition_config_path(&directory_path_data_dir)).unwrap();
    fs::set_permissions(
        custody_composition_config_path(&directory_path_data_dir),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(
        load_runtime_custody_composition_config(&directory_path_data_dir)
            .err()
            .expect("expected non-regular-file rejection")
            .to_string()
            .contains("regular file")
    );

    let hard_link_data_dir = temp.path().join("hard-link-data");
    owner_only_dir(&hard_link_data_dir);
    owner_only_dir(&protected_content_root(&hard_link_data_dir));
    let hard_link_source_dir = temp.path().join("hard-link-source");
    owner_only_dir(&hard_link_source_dir);
    let hard_link_source = hard_link_source_dir.join("custody-composition.json");
    fs::write(
        &hard_link_source,
        serde_json::to_vec(&serde_json::to_value(&config).unwrap()).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&hard_link_source, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(
        &hard_link_source,
        custody_composition_config_path(&hard_link_data_dir),
    )
    .unwrap();
    assert!(load_runtime_custody_composition_config(&hard_link_data_dir)
        .err()
        .expect("expected hard-link rejection")
        .to_string()
        .contains("hard-linked"));
}

#[cfg(unix)]
#[test]
fn write_owner_only_bytes_creates_owner_only_atomic_runtime_storage() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let path = data_dir.join("protected-content/runtime-open/demo/viewers/state.json");
    write_owner_only_bytes(&path, b"{\"state\":\"active\"}").unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"{\"state\":\"active\"}");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn write_owner_only_bytes_failed_replace_preserves_previous_runtime_storage_record() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let path = data_dir.join("protected-content/runtime-open/demo/viewers/state.json");
    write_owner_only_bytes(&path, b"{\"state\":\"closed\"}").unwrap();

    let parent = path.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).unwrap();
    let error = write_owner_only_bytes(&path, b"{\"state\":\"active\"}")
        .expect_err("expected atomic write failure");
    assert!(
        error.to_string().contains("Permission denied") || error.to_string().contains("storage")
    );
    assert_eq!(fs::read(&path).unwrap(), b"{\"state\":\"closed\"}");
    let entries = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec!["state.json".to_string()]);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(unix)]
#[test]
fn runtime_custody_viewer_record_load_rejects_oversized_serialized_state() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let viewer_path = super::runtime_viewer_path(
        &data_dir,
        "person:local:oversized-viewer-record",
        digest(0x61),
    );
    write_owner_only_bytes(&viewer_path, &vec![b'x'; (64 * 1024) + 1]).unwrap();

    let error = super::load_runtime_custody_viewer_record(
        &data_dir,
        "person:local:oversized-viewer-record",
        digest(0x61),
    )
    .err()
    .expect("expected oversized viewer record rejection");
    assert!(error
        .to_string()
        .contains("Runtime custody viewer session is unavailable"));
}

#[cfg(unix)]
#[test]
fn runtime_custody_viewer_record_persist_rejects_oversized_serialized_state() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let mut record = super::RuntimeCustodyViewerRecord::from_active_session(
        "person:local:oversized-viewer-record",
        "did:key:z6MkoD3Yk6TBGj1jiL1pDkV8JrjT4bQmQGq7nD5x7WQy3mYQ",
        digest(0x62),
        "content:1234",
        RuntimeSessionBindingV1::new(digest(0x63)).unwrap(),
        &RuntimeViewerSession::from_persisted_parts(
            digest(0x64),
            opaque_handle(0x65),
            media_identity(0x66).encrypted_content().clone(),
            RightsActionV1::View,
            crate::auth::now_ts() + 60,
        )
        .unwrap(),
        crate::auth::now_ts(),
    )
    .unwrap();
    record.content_id = format!("content:{}", "a".repeat((64 * 1024) + 1));

    let error = super::persist_runtime_custody_viewer_record(
        &data_dir,
        "person:local:oversized-viewer-record",
        digest(0x62),
        &record,
    )
    .expect_err("expected oversized viewer record rejection");
    assert!(error
        .to_string()
        .contains("Runtime custody viewer session is unavailable"));
}

#[cfg(unix)]
#[test]
fn runtime_mint_journal_uses_exact_owner_only_root_and_contains_no_route_or_signed_config() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&protected_content_root(&data_dir));
    let config = custody_composition_config(
        crate::auth::now_ts(),
        custody_route_bindings(
            &signed_custody_epoch(),
            [
                RuntimeCustodyRouteTransportConfig::Local,
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xee),
                },
                RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                    peer_did: peer_did_for_seed(0xef),
                },
            ],
        ),
    );
    write_owner_only_custody_composition_config(&data_dir, &config);
    let loaded = load_runtime_custody_composition_config(&data_dir)
        .unwrap()
        .unwrap();
    let configured_peer_did = match &loaded.routes[1].transport {
        ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
            peer_did, ..
        }) => peer_did.clone(),
        _ => panic!("expected carrier route"),
    };

    let journal = runtime_mint_journal(&data_dir);
    let draft = mint_draft_for_composition_journal_test();
    let persisted = journal.persist_bound(&draft).unwrap();
    assert_eq!(persisted.draft().mint_id(), draft.mint_id());

    let mint_root = protected_content_root(&data_dir).join("runtime-mint");
    assert!(mint_root.is_dir());
    assert_eq!(
        fs::metadata(&mint_root).unwrap().permissions().mode() & 0o777,
        0o700
    );

    assert!(!any_file_contains(
        &mint_root,
        base64::engine::general_purpose::STANDARD
            .decode(&config.signed_pool_base64)
            .unwrap()
            .as_slice()
    ));
    assert!(!any_file_contains(
        &mint_root,
        base64::engine::general_purpose::STANDARD
            .decode(&config.signed_epoch_base64)
            .unwrap()
            .as_slice()
    ));
    assert!(!any_file_contains(
        &mint_root,
        base64::engine::general_purpose::STANDARD
            .decode(&config.signed_committee_authorization_base64)
            .unwrap()
            .as_slice()
    ));
    assert!(!any_file_contains(
        &mint_root,
        configured_peer_did.as_bytes()
    ));
}

#[cfg(unix)]
fn read_pids(path: &Path) -> Vec<u32> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| line.trim().parse().unwrap())
        .collect()
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[tokio::test]
async fn runtime_invokes_custody_not_the_provisional_key_route() {
    let registry = Arc::new(ProviderRegistry::new());
    let custody = RecordingProvider::new(
        "custody",
        json!({"status": "ok", "data": {"echo": "custody"}}),
    );
    let key = RecordingProvider::new("key", json!({"status": "ok", "data": {"echo": "key"}}));
    registry
        .register_sub_provider("key", key.clone())
        .await
        .unwrap();
    register_inactive_custody_runtime_provider_target(registry.as_ref(), custody.clone())
        .await
        .unwrap();

    let data = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({"op": "release_contribution"}),
    )
    .await
    .unwrap();
    assert_eq!(data["echo"], "custody");

    let custody_requests = custody.requests().await;
    let key_requests = key.requests().await;
    assert_eq!(custody_requests.len(), 1);
    assert!(key_requests.is_empty());
    assert_eq!(
        custody_requests[0]["_runtime_invocation"]["source"],
        RUNTIME_PROVIDER_ID
    );
    assert_eq!(
        custody_requests[0]["_runtime_invocation"]["target"],
        CUSTODY_PROVIDER_ID
    );
    assert_eq!(
        custody_requests[0]["_runtime_invocation"]["carrier"],
        Value::Null
    );
    assert_eq!(
        custody_requests[0]["_runtime_invocation"]["transport"],
        "runtime-local-provider-plane"
    );
}

#[tokio::test]
async fn runtime_custody_invoke_fails_closed_on_provider_errors() {
    let registry = Arc::new(ProviderRegistry::new());
    let custody = RecordingProvider::new(
        "custody",
        json!({"status": "error", "code": "invalid_request"}),
    );
    register_inactive_custody_runtime_provider_target(registry.as_ref(), custody)
        .await
        .unwrap();
    let err = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({"op": "release_contribution"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("rejected the request"));
}

#[test]
fn unresolved_release_scan_is_empty_without_creating_state() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    std::fs::create_dir(&data_dir).unwrap();
    assert!(list_unresolved_runtime_releases(&data_dir)
        .unwrap()
        .is_empty());
    assert!(unresolved_release_audit_records(&data_dir)
        .unwrap()
        .is_empty());
    assert!(!data_dir.join("protected-content").exists());
}

#[tokio::test]
async fn inactive_custody_registration_does_not_replace_key() {
    let registry = Arc::new(ProviderRegistry::new());
    let custody = RecordingProvider::new(
        "custody",
        json!({"status": "ok", "data": {"echo": "custody"}}),
    );
    let key = RecordingProvider::new("key", json!({"status": "ok", "data": {"echo": "key"}}));
    registry.register_sub_provider("key", key).await.unwrap();
    register_inactive_custody_runtime_provider_target(&registry, custody)
        .await
        .unwrap();
    let mut schemes = registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, vec!["key".to_string()]);
}

#[tokio::test]
async fn runtime_rights_adapter_invokes_chain_not_the_provisional_rights_route() {
    let registry = Arc::new(ProviderRegistry::new());
    let chain = RecordingProvider::new(
        "chain",
        json!({
            "status": "ok",
            "data": {
                "schema": "elastos.chain.protected-content-rights-evidence/v1"
            }
        }),
    );
    let rights = RecordingProvider::new(
        "rights",
        json!({"status": "ok", "data": {"echo": "rights"}}),
    );
    registry
        .register_sub_provider("rights", rights.clone())
        .await
        .unwrap();
    registry
        .register_sub_provider("chain", chain.clone())
        .await
        .unwrap();

    let data = invoke_json_provider(
        registry.as_ref(),
        CHAIN_PROVIDER_ID,
        CHAIN_RIGHTS_EVIDENCE_OP,
        json!({
            "op": CHAIN_RIGHTS_EVIDENCE_OP,
            "signed_runtime_release_operation": "0xab"
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        data["schema"],
        "elastos.chain.protected-content-rights-evidence/v1"
    );

    let chain_requests = chain.requests().await;
    let rights_requests = rights.requests().await;
    assert_eq!(chain_requests.len(), 1);
    assert!(rights_requests.is_empty());
    assert_eq!(chain_requests[0]["op"], CHAIN_RIGHTS_EVIDENCE_OP);
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["source"],
        RUNTIME_PROVIDER_ID
    );
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["target"],
        CHAIN_PROVIDER_ID
    );
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["carrier"],
        Value::Null
    );
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["transport"],
        "runtime-local-provider-plane"
    );
    assert!(chain_requests[0].get("network").is_none());
    assert!(chain_requests[0].get("rpc_url").is_none());
    assert!(chain_requests[0].get("host").is_none());
    assert!(chain_requests[0].get("has_access").is_none());

    let mut schemes = registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, vec!["chain".to_string(), "rights".to_string()]);
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_registration_passes_only_base_path_and_no_extra_truth() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let state_root = inactive_custody_state_root(&data_dir);
    owner_only_dir(&state_root);
    let (binary, _pid_file, request_log) = write_mock_custody_provider(temp.path());
    let registry = Arc::new(ProviderRegistry::new());

    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();
    assert!(registry.sub_provider_schemes().await.is_empty());
    assert!(
        registry
            .has_ready_runtime_provider_target(CUSTODY_PROVIDER_ID)
            .await
    );

    let response = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({"op": "release_contribution"}),
    )
    .await
    .unwrap();
    assert_eq!(response["echo"], "custody");

    let mut requests = fs::read_to_string(&request_log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 3);
    let init: Value = serde_json::from_str(&requests.remove(0)).unwrap();
    assert_eq!(init["op"], "init");
    assert_eq!(
        init["config"]["base_path"],
        Value::String(state_root.to_string_lossy().to_string())
    );
    assert_eq!(init["config"]["allowed_paths"], json!([]));
    assert_eq!(init["config"]["read_only"], false);
    assert_eq!(init["config"]["encryption_key"], "");
    assert!(init["config"]["extra"].is_null());
    let status: Value = serde_json::from_str(&requests.remove(0)).unwrap();
    assert_eq!(status, json!({"op": "status"}));
    let forwarded: Value = serde_json::from_str(&requests.remove(0)).unwrap();
    assert_eq!(forwarded["op"], "release_contribution");
    assert!(forwarded.get("_runtime_invocation").is_none());
    assert!(forwarded.get("_runtime_transfer").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_wrapper_rejects_public_private_op_and_evidence_injection() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&inactive_custody_state_root(&data_dir));
    let (binary, _pid_file, request_log) = write_mock_custody_provider(temp.path());
    let registry = Arc::new(ProviderRegistry::new());

    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();

    let private_op_error = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "evaluate_rights",
        json!({ "op": "evaluate_rights" }),
    )
    .await
    .unwrap_err();
    assert!(private_op_error.contains("invocation failed"));

    let evidence_error = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({
            "op": "release_contribution",
            "chain_data": {"schema": "forbidden"},
        }),
    )
    .await
    .unwrap_err();
    assert!(evidence_error.contains("invocation failed"));

    let extra_field_error = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({
            "op": "release_contribution",
            "unexpected": true,
        }),
    )
    .await
    .unwrap_err();
    assert!(extra_field_error.contains("rejected the request"));

    let carrier_error = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({
            "op": "release_contribution",
            "carrier": {"ticket": "forbidden"},
        }),
    )
    .await
    .unwrap_err();
    assert!(carrier_error.contains("invocation failed"));

    let requests = fs::read_to_string(&request_log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 3);
    let init: Value = serde_json::from_str(&requests[0]).unwrap();
    assert_eq!(init["op"], "init");
    let status: Value = serde_json::from_str(&requests[1]).unwrap();
    assert_eq!(status, json!({"op": "status"}));
    let forwarded: Value = serde_json::from_str(&requests[2]).unwrap();
    assert_eq!(forwarded["op"], "release_contribution");
    assert_eq!(forwarded["unexpected"], true);
    assert!(forwarded.get("_runtime_invocation").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_wrapper_status_surface_matches_public_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&inactive_custody_state_root(&data_dir));
    let (binary, _pid_file, _request_log) = write_mock_custody_provider(temp.path());
    let registry = Arc::new(ProviderRegistry::new());

    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();

    let status = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "status",
        json!({
            "op": "status",
        }),
    )
    .await
    .unwrap();

    assert_eq!(status["provider"], "custody");
    assert_eq!(status["version"], CUSTODY_PROVIDER_VERSION);
    assert_eq!(status["configured"], Value::Bool(true));
    assert_eq!(
        status["supported_operations"],
        json!([
            "status",
            "provision_node_share",
            "release_contribution",
            "evaluate"
        ])
    );
    assert!(status["supported_operations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|value| value != "shutdown"
            && value != "prepare_evidence"
            && value != "settle_evidence"));
    assert_eq!(status["request_schema"], CUSTODY_PROVIDER_REQUEST_SCHEMA_V1);
    assert_eq!(
        status["response_schema"],
        CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1
    );
}

#[cfg(unix)]
fn inactive_custody_runtime_envelope(op: &str, transport: &str) -> Value {
    let carrier = if transport == "carrier-provider-plane" {
        let endpoint = iroh::SecretKey::from_bytes(&[0x51; 32]).public();
        json!({
            "source_endpoint_did": crate::carrier::public_key_to_did(&endpoint)
                .expect("test endpoint must have a canonical DID"),
        })
    } else {
        Value::Null
    };
    json!({
        "schema": "elastos.provider.invocation/v1",
        "source": RUNTIME_PROVIDER_ID,
        "target": CUSTODY_PROVIDER_ID,
        "op": op,
        "capability": format!("provider:{RUNTIME_PROVIDER_ID}->{CUSTODY_PROVIDER_ID}:{op}"),
        "transport": transport,
        "carrier": carrier,
        "transfer": "json",
        "range": Value::Null,
        "progress": Value::Null,
        "abi": {
            "schema": "elastos.provider.transfer-abi/v1",
            "transfer": "json",
            "transport": transport,
            "range_supported": false,
            "progress_supported": false,
            "progress_mode": "none",
            "transport_native_stream": false,
            "backpressure": "not_applicable",
            "cancel_supported": false,
        }
    })
}

#[cfg(unix)]
async fn spawn_inactive_custody_wrapper_for_test(
    binary: &Path,
    data_dir: &Path,
) -> InactiveCustodyProvider {
    let state_root = inactive_custody_state_root(data_dir);
    let bridge = ProviderBridge::spawn(
        binary,
        ProviderConfig {
            base_path: state_root.to_str().unwrap().to_owned(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    InactiveCustodyProvider::new(
        Arc::new(bridge),
        Arc::downgrade(&Arc::new(ProviderRegistry::new())),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_wrapper_accepts_local_and_carrier_envelopes() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&inactive_custody_state_root(&data_dir));
    let (binary, _pid_file, request_log) = write_mock_custody_provider(temp.path());
    let provider = spawn_inactive_custody_wrapper_for_test(&binary, &data_dir).await;

    for transport in ["runtime-local-provider-plane", "carrier-provider-plane"] {
        let response = provider
            .send_raw(&json!({
                "op": "release_contribution",
                "_runtime_invocation": inactive_custody_runtime_envelope(
                    "release_contribution",
                    transport,
                ),
            }))
            .await
            .unwrap();
        assert_eq!(response["status"], "ok");
    }

    let requests = fs::read_to_string(&request_log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 3);
    for forwarded in requests.iter().skip(1) {
        let forwarded: Value = serde_json::from_str(forwarded).unwrap();
        assert_eq!(forwarded["op"], "release_contribution");
        assert!(forwarded.get("_runtime_invocation").is_none());
        assert!(forwarded.get("carrier").is_none());
        assert!(!forwarded.to_string().contains("source_endpoint_did"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_wrapper_rejects_invalid_transport_and_injected_carrier_data() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&inactive_custody_state_root(&data_dir));
    let (binary, _pid_file, _request_log) = write_mock_custody_provider(temp.path());
    let provider = spawn_inactive_custody_wrapper_for_test(&binary, &data_dir).await;

    let invalid_transport = provider
        .send_raw(&json!({
            "op": "release_contribution",
            "_runtime_invocation": inactive_custody_runtime_envelope(
                "release_contribution",
                "unsupported-provider-plane",
            ),
        }))
        .await
        .unwrap_err();
    assert!(invalid_transport
        .to_string()
        .contains("runtime envelope is invalid"));

    let injected_carrier = provider
        .send_raw(&json!({
            "op": "release_contribution",
            "_runtime_invocation": inactive_custody_runtime_envelope(
                "release_contribution",
                "carrier-provider-plane",
            ),
            "carrier": {"ticket": "forbidden"},
        }))
        .await
        .unwrap_err();
    assert!(injected_carrier
        .to_string()
        .contains("unsupported injected carrier data"));

    for (name, transport, carrier) in [
        (
            "missing authenticated Carrier endpoint",
            "carrier-provider-plane",
            Value::Null,
        ),
        (
            "invalid Carrier endpoint DID",
            "carrier-provider-plane",
            json!({"source_endpoint_did": "did:key:invalid"}),
        ),
        (
            "extra Carrier metadata",
            "carrier-provider-plane",
            json!({
                "source_endpoint_did": crate::carrier::public_key_to_did(
                    &iroh::SecretKey::from_bytes(&[0x52; 32]).public(),
                )
                .unwrap(),
                "route": "forbidden",
            }),
        ),
        (
            "Carrier metadata on a local request",
            "runtime-local-provider-plane",
            json!({
                "source_endpoint_did": crate::carrier::public_key_to_did(
                    &iroh::SecretKey::from_bytes(&[0x53; 32]).public(),
                )
                .unwrap(),
            }),
        ),
    ] {
        let mut envelope = inactive_custody_runtime_envelope("release_contribution", transport);
        envelope["carrier"] = carrier;
        let error = provider
            .send_raw(&json!({
                "op": "release_contribution",
                "_runtime_invocation": envelope,
            }))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("runtime envelope is invalid"),
            "{name}: {error}"
        );
    }
}

#[tokio::test]
async fn runtime_custody_registry_adapter_invokes_selected_custody_endpoint_for_rights() {
    let registry = Arc::new(ProviderRegistry::new());
    let envelope = custody_envelope_for_media(0x11);
    let operation = make_signed_runtime_release_operation_for_envelope_and_seed(0x21, &envelope);
    let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
    let custody = RecordingProvider::new(
        "custody",
        ok_provider_response(
            serde_json::to_value(RightsProviderResponseV1::new_decision(&decision).unwrap())
                .unwrap(),
        ),
    );
    let provisional = RecordingProvider::new(
        "rights",
        ok_provider_response(json!({"echo": "provisional"})),
    );
    registry
        .register_runtime_provider_target(CUSTODY_PROVIDER_ID, custody.clone())
        .await
        .unwrap();
    registry
        .register_sub_provider("rights", provisional.clone())
        .await
        .unwrap();

    let adapter = RuntimeCustodyRegistryAdapter::new(
        registry.clone(),
        elastos_runtime::provider::ProviderInvocationTransport::Local,
    );
    let request = RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();

    let response = adapter.evaluate_rights(&request).await.unwrap();
    assert_eq!(
        response.status(),
        elastos_protected_content_provider_contracts::RightsProviderResponseStatusV1::Decision
    );
    let custody_requests = custody.requests().await;
    assert_eq!(custody_requests.len(), 1);
    assert!(provisional.requests().await.is_empty());
    let recorded = &custody_requests[0];
    assert_eq!(recorded["op"], "evaluate");
    assert_eq!(
        recorded["_runtime_invocation"]["source"],
        RUNTIME_PROVIDER_ID
    );
    assert_eq!(
        recorded["_runtime_invocation"]["target"],
        CUSTODY_PROVIDER_ID
    );
    assert_eq!(
        recorded["_runtime_invocation"]["transport"],
        "runtime-local-provider-plane"
    );
    assert_eq!(recorded["_runtime_invocation"]["carrier"], Value::Null);
    assert!(recorded.get("chain_data").is_none());
    assert!(recorded.get("evidence").is_none());
    let debug = format!("{adapter:?}");
    assert!(!debug.contains("runtime-local-provider-plane"));
    assert!(!debug.contains("ProviderRegistry"));
}

#[tokio::test]
async fn runtime_custody_registry_adapter_invokes_selected_custody_endpoint_for_release() {
    let registry = Arc::new(ProviderRegistry::new());
    let envelope = custody_envelope_for_media(0x11);
    let operation = make_signed_runtime_release_operation_for_envelope_and_seed(0x21, &envelope);
    let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
    let contribution = make_signed_node_contribution(&operation, 1);
    let custody = RecordingProvider::new(
        "custody",
        ok_provider_response(
            serde_json::to_value(
                CustodyProviderResponseV1::new_contribution(&contribution).unwrap(),
            )
            .unwrap(),
        ),
    );
    let provisional =
        RecordingProvider::new("key", ok_provider_response(json!({"echo": "provisional"})));
    registry
        .register_runtime_provider_target(CUSTODY_PROVIDER_ID, custody.clone())
        .await
        .unwrap();
    registry
        .register_sub_provider("key", provisional.clone())
        .await
        .unwrap();

    let adapter = RuntimeCustodyRegistryAdapter::new(
        registry.clone(),
        elastos_runtime::provider::ProviderInvocationTransport::Local,
    );
    let request =
        CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();

    let response = adapter.release_contribution(&request).await.unwrap();
    assert_eq!(
        response.status(),
        elastos_protected_content_provider_contracts::CustodyProviderResponseStatusV1::Contribution
    );
    let custody_requests = custody.requests().await;
    assert_eq!(custody_requests.len(), 1);
    assert!(provisional.requests().await.is_empty());
    let recorded = &custody_requests[0];
    assert_eq!(recorded["op"], "release_contribution");
    assert_eq!(
        recorded["_runtime_invocation"]["source"],
        RUNTIME_PROVIDER_ID
    );
    assert_eq!(
        recorded["_runtime_invocation"]["target"],
        CUSTODY_PROVIDER_ID
    );
    assert_eq!(
        recorded["_runtime_invocation"]["transport"],
        "runtime-local-provider-plane"
    );
    assert_eq!(recorded["_runtime_invocation"]["carrier"], Value::Null);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_registry_adapter_process_happy_path_uses_public_provision_receipt() {
    let binary = required_test_binary_path(TEST_CUSTODY_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let temp_root = fs::canonicalize(temp.path()).unwrap();
    let data_dir = temp_root.join("data");
    owner_only_dir(&data_dir);
    let state_root = inactive_custody_state_root(&data_dir);
    owner_only_dir(state_root.parent().unwrap());
    let provisioned = provision_custody_node_public_receipt(&binary, &state_root, 0x21);

    let registry = Arc::new(ProviderRegistry::new());
    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();

    let custody_epoch = signed_custody_epoch_with_first_node(
        provisioned.node_public_key,
        provisioned.node_custody_public_key,
    );
    let provisioning_now = crate::auth::now_ts();
    let envelope =
        provisioned_custody_envelope_for_media_with_epoch(0x11, &custody_epoch, provisioning_now);
    let provisioning_record =
        provisioning_record_for_selected_node(&envelope, provisioned.node_public_key);
    let provisioning =
        signed_runtime_custody_provisioning_at(&provisioning_record, 0x21, provisioning_now);
    let adapter = RuntimeCustodyRegistryAdapter::new(
        registry.clone(),
        elastos_runtime::provider::ProviderInvocationTransport::Local,
    );

    let provision_request =
        CustodyProviderRequestV1::new_provision_node_share(&provisioning_record, &provisioning)
            .unwrap();
    let provision_response = adapter
        .provision_node_share(&provision_request)
        .await
        .unwrap();
    assert_eq!(
        provision_response.provisioned_record_identity().unwrap(),
        provisioning_record.record_identity().unwrap()
    );

    let operation_now = crate::auth::now_ts();
    let operation = make_signed_runtime_release_operation_for_envelope_and_epoch_at(
        0x21,
        &envelope,
        custody_epoch.clone(),
        operation_now,
    );
    let rights_request =
        RightsProviderRequestV1::new_evaluate(provisioned.node_public_key, &operation).unwrap();
    let chain = ProcessChainEvidenceProvider::new(rights_request.clone(), true);
    registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain.clone())
        .await
        .unwrap();

    let rights_validation_now = crate::auth::now_ts();
    let validated_rights = match ValidatedRightsProviderRequestV1::decode_and_validate_at(
        &rights_request.to_json_vec().unwrap(),
        runtime_operation_issuer_for_seed(0x21),
        rights_validation_now,
    ) {
        Ok(validated) => validated,
        Err(error) => panic!("local rights request validation failed: {error:?}"),
    };
    assert_eq!(
        validated_rights.selected_node_public_key(),
        provisioned.node_public_key
    );

    let rights_response = adapter.evaluate_rights(&rights_request).await.unwrap();
    let decision = rights_response.signed_node_rights_decision().unwrap();
    let release_request =
        CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
    let release_validation_now = crate::auth::now_ts();
    let validated_release = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
        &release_request.to_json_vec().unwrap(),
        runtime_operation_issuer_for_seed(0x21),
        provisioned.node_public_key,
        release_validation_now,
    )
    .unwrap();
    let validated_release = validated_release.release_contribution().unwrap();
    let authenticated_operation = operation
        .verify(
            operation.statement().runtime_operation_issuer(),
            release_validation_now,
        )
        .unwrap();
    assert_eq!(
        validated_release.authenticated_runtime_release_operation(),
        &authenticated_operation
    );
    assert_eq!(validated_release.signed_node_rights_decision(), &decision);
    let release_response = adapter
        .release_contribution(&release_request)
        .await
        .unwrap();
    if release_response.status()
        == elastos_protected_content_provider_contracts::CustodyProviderResponseStatusV1::Failure
    {
        panic!(
            "release provider failure: {:?}",
            release_response.failure_code().unwrap()
        );
    }
    let contribution = release_response.signed_node_contribution().unwrap();

    let verification_now = crate::auth::now_ts();
    let authenticated = operation
        .verify(
            operation.statement().runtime_operation_issuer(),
            verification_now,
        )
        .unwrap();
    let node_set = custody_epoch.statement().node_set().unwrap();
    authenticated
        .verify_node_contribution(&contribution, &node_set, verification_now)
        .unwrap();

    let chain_requests = chain.requests().await;
    assert_eq!(chain_requests.len(), 1);
    assert_eq!(chain_requests[0]["op"], CHAIN_RIGHTS_EVIDENCE_OP);
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["source"],
        RUNTIME_PROVIDER_ID
    );
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["target"],
        CHAIN_PROVIDER_ID
    );
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["transport"],
        "runtime-local-provider-plane"
    );
    assert_eq!(
        chain_requests[0]["_runtime_invocation"]["carrier"],
        Value::Null
    );
    let mut inner_chain_request = chain_requests[0].clone();
    inner_chain_request
        .as_object_mut()
        .unwrap()
        .remove("_runtime_invocation");
    assert_eq!(
        inner_chain_request,
        chain_rights_evidence_request(&operation).unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_release_coordinator_process_two_of_three_success_stops_before_third_node() {
    let binary = required_test_binary_path(TEST_CUSTODY_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let temp_root = fs::canonicalize(temp.path()).unwrap();

    let node1 = provisioned_process_custody_node(&binary, &temp_root, "node-1", 0x21, digest(0xa1));
    let node2 = provisioned_process_custody_node(&binary, &temp_root, "node-2", 0x21, digest(0xa2));
    let node3 = provisioned_process_custody_node(&binary, &temp_root, "node-3", 0x21, digest(0xa3));

    register_inactive_custody_provider(&node1.registry, &binary, &temp_root.join("node-1"))
        .await
        .unwrap();
    register_inactive_custody_provider(&node2.registry, &binary, &temp_root.join("node-2"))
        .await
        .unwrap();
    register_inactive_custody_provider(&node3.registry, &binary, &temp_root.join("node-3"))
        .await
        .unwrap();

    let custody_epoch = signed_custody_epoch_for_node_keys([
        (
            node1.provisioned.node_public_key,
            node1.provisioned.node_custody_public_key,
        ),
        (
            node2.provisioned.node_public_key,
            node2.provisioned.node_custody_public_key,
        ),
        (
            node3.provisioned.node_public_key,
            node3.provisioned.node_custody_public_key,
        ),
    ]);
    let fixtures_by_node = BTreeMap::from([
        (node1.provisioned.node_public_key, node1),
        (node2.provisioned.node_public_key, node2),
        (node3.provisioned.node_public_key, node3),
    ]);
    let ordered_node_keys = custody_epoch
        .statement()
        .nodes()
        .iter()
        .map(|node| node.node_public_key())
        .collect::<Vec<_>>();
    let ordered_fixtures = ordered_node_keys
        .iter()
        .map(|node_public_key| fixtures_by_node.get(node_public_key).unwrap())
        .collect::<Vec<_>>();
    let provisioning_now = crate::auth::now_ts();
    let envelope =
        provisioned_custody_envelope_for_media_with_epoch(0x11, &custody_epoch, provisioning_now);
    for fixture in fixtures_by_node.values() {
        provision_selected_node_share(fixture, &envelope, 0x21, provisioning_now).await;
    }

    let operation_now = crate::auth::now_ts();
    let operation = make_signed_runtime_release_operation_for_envelope_and_epoch_at(
        0x21,
        &envelope,
        custody_epoch.clone(),
        operation_now,
    );
    let rights_request_1 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[0].provisioned.node_public_key,
        &operation,
    )
    .unwrap();
    let rights_request_2 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[1].provisioned.node_public_key,
        &operation,
    )
    .unwrap();
    let rights_request_3 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[2].provisioned.node_public_key,
        &operation,
    )
    .unwrap();
    let chain1 = ProcessChainEvidenceProvider::new(rights_request_1, true);
    let chain2 = ProcessChainEvidenceProvider::new(rights_request_2, true);
    let chain3 = ProcessChainEvidenceProvider::new(rights_request_3, true);
    ordered_fixtures[0]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain1.clone())
        .await
        .unwrap();
    ordered_fixtures[1]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain2.clone())
        .await
        .unwrap();
    ordered_fixtures[2]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain3.clone())
        .await
        .unwrap();

    let runtime_owner_parent = temp_root.join("runtime-owner-only-parent");
    owner_only_dir(&runtime_owner_parent);
    let runtime_data_dir = runtime_owner_parent.join("runtime-release");
    let coordinator = RuntimeReleaseCoordinator::new(
        RuntimeReleaseJournal::new(runtime_data_dir.clone()),
        runtime_operation_issuer_for_seed(0x21),
        vec![
            RuntimeSelectedProvider::new(
                ordered_fixtures[0].provisioned.node_public_key,
                &ordered_fixtures[0].adapter,
                &ordered_fixtures[0].adapter,
            ),
            RuntimeSelectedProvider::new(
                ordered_fixtures[1].provisioned.node_public_key,
                &ordered_fixtures[1].adapter,
                &ordered_fixtures[1].adapter,
            ),
            RuntimeSelectedProvider::new(
                ordered_fixtures[2].provisioned.node_public_key,
                &ordered_fixtures[2].adapter,
                &ordered_fixtures[2].adapter,
            ),
        ],
    )
    .unwrap()
    .with_response_clock(crate::auth::now_ts);
    let wallet_now = crate::auth::now_ts();
    let (wallet_request, wallet_response) = wallet_request_response_for_release_at(
        &operation,
        "profile:alpha",
        "wallet-account-alpha",
        "wallet-request:11111111111111111111111111111111",
        wallet_now,
    );

    let outcome = coordinator
        .release(
            &wallet_request,
            &wallet_response,
            operation.clone(),
            crate::auth::now_ts(),
        )
        .await
        .unwrap();
    let signed_node_contributions = match outcome {
        RuntimeReleaseCoordinatorOutcome::Terminal(
            RuntimeReleaseTerminalResult::ContributionsReady {
                signed_node_contributions,
            },
        ) => signed_node_contributions,
        other => panic!("unexpected coordinator outcome: {other:?}"),
    };
    assert_eq!(signed_node_contributions.len(), 2);
    let verification_now = crate::auth::now_ts();
    let authenticated = operation
        .verify(
            operation.statement().runtime_operation_issuer(),
            verification_now,
        )
        .unwrap();
    let node_set = custody_epoch.statement().node_set().unwrap();
    for contribution in &signed_node_contributions {
        authenticated
            .verify_node_contribution(contribution, &node_set, verification_now)
            .unwrap();
    }
    let contributing_nodes: Vec<_> = signed_node_contributions
        .iter()
        .map(|contribution| {
            contribution
                .statement()
                .signed_rights_decision()
                .statement()
                .node_public_key()
        })
        .collect();
    assert_eq!(
        contributing_nodes,
        vec![
            ordered_fixtures[0].provisioned.node_public_key,
            ordered_fixtures[1].provisioned.node_public_key,
        ]
    );
    assert_eq!(chain1.requests().await.len(), 1);
    assert_eq!(chain2.requests().await.len(), 1);
    assert_eq!(chain3.requests().await.len(), 0);
    assert!(list_unresolved_runtime_releases(&runtime_data_dir)
        .unwrap()
        .is_empty());
    assert!(unresolved_release_audit_records(&runtime_data_dir)
        .unwrap()
        .is_empty());
    let terminal_debug = format!(
        "{:?}",
        RuntimeReleaseTerminalResult::ContributionsReady {
            signed_node_contributions,
        }
    );
    assert!(!terminal_debug.contains("node-1"));
    assert!(!terminal_debug.contains("node-2"));
    assert!(!terminal_debug.contains("node-3"));
    assert!(!terminal_debug.contains("carrier-provider-plane"));

    ordered_fixtures[0]
        .registry
        .unregister_sub_provider(CHAIN_PROVIDER_ID)
        .await
        .unwrap();
    ordered_fixtures[1]
        .registry
        .unregister_sub_provider(CHAIN_PROVIDER_ID)
        .await
        .unwrap();
    ordered_fixtures[2]
        .registry
        .unregister_sub_provider(CHAIN_PROVIDER_ID)
        .await
        .unwrap();

    let beta_now = crate::auth::now_ts();
    let beta_operation =
        make_signed_runtime_release_operation_for_envelope_and_epoch_and_recipient_at(
            0x21,
            &envelope,
            custody_epoch.clone(),
            recipient_public_key(0x31),
            recipient_identity(0x31),
            RuntimeReleaseAuditIdV1::new(digest(0xb2)).unwrap(),
            beta_now,
        );
    let (beta_wallet_request, beta_wallet_response) =
        wallet_request_response_for_release_context_at(
            &beta_operation,
            "profile:beta",
            "runtime-session:beta",
            Some("proof:beta"),
            "grant:beta",
            "launch:beta",
            "wallet-account-beta",
            "wallet-request:22222222222222222222222222222222",
            beta_now,
        );
    let beta_rights_request_1 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[0].provisioned.node_public_key,
        &beta_operation,
    )
    .unwrap();
    let beta_rights_request_2 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[1].provisioned.node_public_key,
        &beta_operation,
    )
    .unwrap();
    let beta_rights_request_3 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[2].provisioned.node_public_key,
        &beta_operation,
    )
    .unwrap();
    let beta_chain1 = ProcessChainEvidenceProvider::new(beta_rights_request_1, false);
    let beta_chain2 = ProcessChainEvidenceProvider::new(beta_rights_request_2, false);
    let beta_chain3 = ProcessChainEvidenceProvider::new(beta_rights_request_3, false);
    ordered_fixtures[0]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, beta_chain1.clone())
        .await
        .unwrap();
    ordered_fixtures[1]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, beta_chain2.clone())
        .await
        .unwrap();
    ordered_fixtures[2]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, beta_chain3.clone())
        .await
        .unwrap();

    let beta_outcome = coordinator
        .release(
            &beta_wallet_request,
            &beta_wallet_response,
            beta_operation.clone(),
            crate::auth::now_ts(),
        )
        .await
        .unwrap();
    let beta_denied = match beta_outcome {
        RuntimeReleaseCoordinatorOutcome::Terminal(
            RuntimeReleaseTerminalResult::RightsDenied {
                signed_node_rights_decision,
            },
        ) => signed_node_rights_decision,
        other => panic!("unexpected beta coordinator outcome: {other:?}"),
    };
    assert_eq!(beta_denied.statement().decision(), RightsDecisionV1::Denied);
    assert_eq!(
        beta_denied.statement().node_public_key(),
        ordered_fixtures[0].provisioned.node_public_key
    );
    assert_eq!(beta_chain1.requests().await.len(), 1);
    assert_eq!(beta_chain2.requests().await.len(), 0);
    assert_eq!(beta_chain3.requests().await.len(), 0);

    let beta_replay = coordinator
        .release(
            &beta_wallet_request,
            &beta_wallet_response,
            beta_operation.clone(),
            crate::auth::now_ts(),
        )
        .await
        .unwrap();
    let beta_replay_denied = match beta_replay {
        RuntimeReleaseCoordinatorOutcome::Terminal(
            RuntimeReleaseTerminalResult::RightsDenied {
                signed_node_rights_decision,
            },
        ) => signed_node_rights_decision,
        other => panic!("unexpected beta replay outcome: {other:?}"),
    };
    assert_eq!(beta_replay_denied, beta_denied);
    assert_eq!(beta_chain1.requests().await.len(), 1);
    assert_eq!(beta_chain2.requests().await.len(), 0);
    assert_eq!(beta_chain3.requests().await.len(), 0);

    let beta_persisted = RuntimeReleaseJournal::new(runtime_data_dir.clone())
        .load(beta_operation.canonical_hash().unwrap())
        .unwrap();
    assert!(matches!(
        beta_persisted.terminal_result(),
        Some(RuntimeReleaseTerminalResult::RightsDenied { .. })
    ));
    assert_eq!(
        beta_persisted
            .terminal_result()
            .unwrap()
            .contribution_count(),
        0
    );
    assert!(list_unresolved_runtime_releases(&runtime_data_dir)
        .unwrap()
        .is_empty());
    assert!(unresolved_release_audit_records(&runtime_data_dir)
        .unwrap()
        .is_empty());
    let beta_terminal_debug = format!(
        "{:?}",
        RuntimeReleaseTerminalResult::RightsDenied {
            signed_node_rights_decision: beta_denied.clone(),
        }
    );
    assert!(!beta_terminal_debug.contains("node-1"));
    assert!(!beta_terminal_debug.contains("node-2"));
    assert!(!beta_terminal_debug.contains("node-3"));
    assert!(!beta_terminal_debug.contains("carrier-provider-plane"));

    for fixture in ordered_fixtures {
        fixture
            .registry
            .unregister_sub_provider(CHAIN_PROVIDER_ID)
            .await
            .unwrap();
        fixture
            .registry
            .unregister_runtime_provider_target(CUSTODY_PROVIDER_ID)
            .await
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_decrypt_registry_adapter_process_reconstructs_for_prepared_recipient_and_closes_cleanly(
) {
    let custody_binary = required_test_binary_path(TEST_CUSTODY_PROVIDER_BIN_ENV);
    let decrypt_binary = required_test_binary_path(TEST_DECRYPT_PROVIDER_BIN_ENV);
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let temp_root = fs::canonicalize(temp.path()).unwrap();

    let node1 =
        provisioned_process_custody_node(&custody_binary, &temp_root, "node-1", 0x21, digest(0xa1));
    let node2 =
        provisioned_process_custody_node(&custody_binary, &temp_root, "node-2", 0x21, digest(0xa2));
    let node3 =
        provisioned_process_custody_node(&custody_binary, &temp_root, "node-3", 0x21, digest(0xa3));

    register_inactive_custody_provider(&node1.registry, &custody_binary, &temp_root.join("node-1"))
        .await
        .unwrap();
    register_inactive_custody_provider(&node2.registry, &custody_binary, &temp_root.join("node-2"))
        .await
        .unwrap();
    register_inactive_custody_provider(&node3.registry, &custody_binary, &temp_root.join("node-3"))
        .await
        .unwrap();

    let custody_epoch = signed_custody_epoch_for_node_keys([
        (
            node1.provisioned.node_public_key,
            node1.provisioned.node_custody_public_key,
        ),
        (
            node2.provisioned.node_public_key,
            node2.provisioned.node_custody_public_key,
        ),
        (
            node3.provisioned.node_public_key,
            node3.provisioned.node_custody_public_key,
        ),
    ]);
    let fixtures_by_node = BTreeMap::from([
        (node1.provisioned.node_public_key, node1),
        (node2.provisioned.node_public_key, node2),
        (node3.provisioned.node_public_key, node3),
    ]);
    let ordered_node_keys = custody_epoch
        .statement()
        .nodes()
        .iter()
        .map(|node| node.node_public_key())
        .collect::<Vec<_>>();
    let ordered_fixtures = ordered_node_keys
        .iter()
        .map(|node_public_key| fixtures_by_node.get(node_public_key).unwrap())
        .collect::<Vec<_>>();

    let provisioning_now = crate::auth::now_ts();
    let signed_pool = signed_custody_pool_for_epoch(
        &custody_epoch,
        (provisioning_now.saturating_sub(10), provisioning_now + 10),
    );
    let signed_committee_authorization = signed_committee_authorization_for_epoch(
        signed_pool.pool_identity().unwrap(),
        &custody_epoch,
    );
    let (clear_init_segment, clear_segments) = clear_media_components(0x11);
    let protect_registry = Arc::new(ProviderRegistry::new());
    let protect_bridge = ProviderBridge::spawn(
        &protect_binary,
        ProviderConfig {
            extra: json!({}),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let protect_provider: Arc<dyn Provider> = Arc::new(CapsuleProvider::with_scheme(
        Arc::new(protect_bridge),
        "protect",
    ));
    protect_registry
        .register_runtime_provider_target(PROTECT_PROVIDER_ID, protect_provider)
        .await
        .unwrap();
    let protect_nodes = ordered_fixtures
        .iter()
        .map(|fixture| {
            ProtectionSessionNodeV1::new(
                fixture.provisioned.node_public_key,
                fixture.provisioned.node_custody_public_key,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let opened = invoke_typed_protect_provider(
        protect_registry.as_ref(),
        "open_protection_session",
        &ProtectProviderRequestV1::new_open_protection_session(
            digest(0xb1),
            content_access_id(0x41),
            signed_pool.pool_identity().unwrap(),
            custody_epoch.epoch_identity().unwrap(),
            signed_committee_authorization
                .authorization_identity()
                .unwrap(),
            MEDIA_MIME_TYPE_V1,
            MEDIA_CODECS_V1,
            u32::try_from(clear_segments.len()).unwrap(),
            &clear_init_segment,
            protect_nodes,
        )
        .unwrap(),
    )
    .await;
    assert_eq!(
        opened.status(),
        ProtectProviderResponseStatusV1::ProtectionSessionOpened
    );
    let protection_session_handle = opened.protection_session_handle().unwrap().unwrap();
    let protected_init_segment = opened.protected_init_segment().unwrap().to_vec();
    let mut encrypted_segments = Vec::with_capacity(clear_segments.len());
    for (segment_index, clear_segment) in clear_segments.iter().enumerate() {
        let protected = invoke_typed_protect_provider(
            protect_registry.as_ref(),
            "protect_media_segment",
            &ProtectProviderRequestV1::new_protect_media_segment(
                protection_session_handle,
                u32::try_from(segment_index).unwrap(),
                clear_segment,
            )
            .unwrap(),
        )
        .await;
        assert_eq!(
            protected.status(),
            ProtectProviderResponseStatusV1::MediaSegmentProtected
        );
        assert_eq!(
            protected.segment_index(),
            Some(u32::try_from(segment_index).unwrap())
        );
        encrypted_segments.push(protected.protected_segment().unwrap().to_vec());
    }
    let finalized = invoke_typed_protect_provider(
        protect_registry.as_ref(),
        "finalize_protection_session",
        &ProtectProviderRequestV1::new_finalize_protection_session(protection_session_handle)
            .unwrap(),
    )
    .await;
    assert_eq!(
        finalized.status(),
        ProtectProviderResponseStatusV1::ProtectionSessionFinalized
    );
    let protected_media_identity = finalized.media_identity().unwrap().unwrap();
    let envelope = finalized.custody_envelope().unwrap().unwrap();
    assert_eq!(
        protected_media_identity,
        CencFmp4MediaIdentityV1::new_from_bytes(
            &protected_init_segment,
            &encrypted_segments,
            MEDIA_MIME_TYPE_V1,
            MEDIA_CODECS_V1,
        )
        .unwrap()
    );
    assert_eq!(
        envelope.manifest().encrypted_content(),
        protected_media_identity.encrypted_content()
    );
    assert_eq!(
        envelope.manifest().custody_pool(),
        signed_pool.pool_identity().unwrap()
    );
    assert_eq!(
        envelope.manifest().custody_epoch(),
        custody_epoch.epoch_identity().unwrap()
    );
    assert_eq!(
        envelope.manifest().custody_committee_authorization(),
        signed_committee_authorization
            .authorization_identity()
            .unwrap()
    );
    assert_eq!(
        envelope.manifest().threshold(),
        ThresholdV1::new(2, 3).unwrap()
    );
    assert_eq!(
        envelope.manifest().nodes(),
        custody_epoch.statement().nodes()
    );
    let protect_closed = invoke_typed_protect_provider(
        protect_registry.as_ref(),
        "close_protection_session",
        &ProtectProviderRequestV1::new_close_protection_session(protection_session_handle).unwrap(),
    )
    .await;
    assert_eq!(
        protect_closed.status(),
        ProtectProviderResponseStatusV1::ProtectionSessionClosed
    );

    let mint_nodes = ordered_fixtures
        .iter()
        .enumerate()
        .map(|(index, fixture)| {
            let node_seed = u8::try_from(index + 1).unwrap();
            RuntimeMintNodeBinding::new(
                fixture.provisioned.node_public_key,
                CustodyPoolOperatorIdV1::new([0x80 + node_seed; 32]),
                CustodyPoolFailureDomainIdV1::new([0x90 + node_seed; 32]),
                fixture.owner_state_root,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let mint_draft = RuntimeMintDraft::new(
        &protected_init_segment,
        &encrypted_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
        content_access_id(0x41),
        envelope.key_envelope_identity().unwrap(),
        policy_body().policy_identity().unwrap(),
        envelope.manifest().content_key_commitment(),
        envelope.manifest().threshold(),
        mint_nodes.clone(),
    )
    .unwrap();
    let mint_owner_parent = temp_root.join("runtime-mint-owner-only-parent");
    owner_only_dir(&mint_owner_parent);
    let mint_root = mint_owner_parent.join("runtime-mint");
    let mint_selected = mint_nodes
        .iter()
        .zip(ordered_fixtures.iter())
        .map(|(binding, fixture)| RuntimeMintSelectedNode::new(binding.clone(), &fixture.adapter))
        .collect::<Vec<_>>();
    let mint_coordinator = RuntimeMintCoordinator::new(
        RuntimeMintJournal::new(mint_root.clone()),
        runtime_operation_issuer_for_seed(0x21),
        sign_runtime_statement_for_seed_0x21,
        mint_selected,
    )
    .unwrap();
    let mint_outcome = mint_coordinator
        .provision(&mint_draft, &envelope, provisioning_now)
        .await
        .unwrap();
    match mint_outcome {
        RuntimeMintCoordinatorOutcome::CustodyProvisioned { mint_id } => {
            assert_eq!(mint_id, mint_draft.mint_id());
        }
        other => panic!("unexpected mint outcome: {other:?}"),
    }
    let mint_journal = RuntimeMintJournal::new(mint_root.clone());
    let custody_provisioned = mint_journal.load(mint_draft.mint_id()).unwrap();
    assert!(custody_provisioned.any_effect_started());
    assert!(custody_provisioned.all_receipts_present());
    assert_eq!(custody_provisioned.accepted_orphans().len(), 3);
    let sealed_share_bytes = envelope.stored_shares()[0].canonical_bytes().unwrap();
    assert!(!any_file_contains(&mint_root, &sealed_share_bytes));
    assert!(!any_file_contains(&mint_root, &protected_init_segment));
    assert!(!any_file_contains(&mint_root, &encrypted_segments[0]));
    assert!(!any_file_contains(&mint_root, &clear_init_segment));
    assert!(!any_file_contains(&mint_root, &clear_segments[0]));

    let content_provider = ContentAvailabilityTestProvider::new(
        0x61,
        ContentAvailabilityTestConfig {
            checked_at: crate::auth::now_ts(),
            ..ContentAvailabilityTestConfig::accepted()
        },
    );
    let requirement = availability_requirement(content_provider.signer_did());
    let content_registry = Arc::new(ProviderRegistry::new());
    content_registry
        .register_sub_provider("content", content_provider)
        .await
        .unwrap();
    let content_directory = protected_content_directory_from_parts(
        &protected_init_segment,
        &encrypted_segments,
        &protected_media_identity,
    );
    let availability_evidence = super::publish_and_verify_protected_content_availability(
        content_registry.as_ref(),
        content_directory.path(),
        mint_draft.media_identity(),
        &requirement,
        crate::auth::now_ts(),
    )
    .await
    .unwrap();
    let availability_outcome = mint_coordinator
        .record_content_availability(&mint_draft, &requirement, availability_evidence)
        .unwrap();
    match availability_outcome {
        RuntimeMintCoordinatorOutcome::ContentAvailable { mint_id } => {
            assert_eq!(mint_id, mint_draft.mint_id());
        }
        other => panic!("unexpected availability outcome: {other:?}"),
    }
    let mint = mint_journal.load(mint_draft.mint_id()).unwrap();
    assert!(mint.content_availability().is_some());

    let purchase_effect = runtime_verified_purchase_effect_for_mint(
        &mint,
        "profile:alpha",
        "wallet-account-alpha",
        "wallet-request:11111111111111111111111111111111",
        0xaa,
        crate::auth::now_ts(),
    );
    let profile = ProfileIdentityV1::from_public_key_bytes(
        SigningKey::from_bytes(&[0x26; 32])
            .verifying_key()
            .to_bytes(),
    )
    .unwrap();
    let session_a = RuntimeSessionBindingV1::new(digest(0x66)).unwrap();
    let session_b = RuntimeSessionBindingV1::new(digest(0x67)).unwrap();
    let preliminary_buy = bind_buy(&mint, "profile:alpha", profile, &purchase_effect).unwrap();
    assert_eq!(
        preliminary_buy.binding_for_session(session_a).unwrap(),
        binding_for_envelope(&envelope)
    );
    assert_eq!(preliminary_buy.action(), RightsActionV1::View);

    let decrypt_registry = Arc::new(ProviderRegistry::new());
    register_protected_content_decrypt_provider(
        &decrypt_registry,
        &decrypt_binary,
        runtime_operation_issuer_for_seed(0x21),
    )
    .await
    .unwrap();
    let decrypt = RuntimeDecryptRegistryAdapter::new(decrypt_registry.clone());

    let audit_a = RuntimeReleaseAuditIdV1::new(digest(0xa1)).unwrap();
    let audit_b = RuntimeReleaseAuditIdV1::new(digest(0xa2)).unwrap();
    let flow_now = crate::auth::now_ts();
    let prepare_issued_at = flow_now.saturating_sub(5);
    let prepare_expires_at = flow_now + 240;
    let prepared_a = prepare_recipient(
        &decrypt,
        &preliminary_buy,
        session_a,
        audit_a,
        runtime_operation_issuer_for_seed(0x21),
        prepare_issued_at,
        prepare_expires_at,
    )
    .await
    .unwrap();
    let prepared_b = prepare_recipient(
        &decrypt,
        &preliminary_buy,
        session_b,
        audit_b,
        runtime_operation_issuer_for_seed(0x21),
        prepare_issued_at,
        prepare_expires_at,
    )
    .await
    .unwrap();
    assert_ne!(prepared_a.binding(), prepared_b.binding());

    let operation = make_signed_runtime_release_operation_for_envelope_and_epoch_and_recipient_at(
        0x21,
        &envelope,
        custody_epoch.clone(),
        *prepared_a.recipient_public_key(),
        prepared_a.recipient_identity().clone(),
        audit_a,
        flow_now,
    );
    assert_eq!(
        prepared_a.binding(),
        operation.statement().rights_request().request().binding()
    );
    let (wallet_request, wallet_response) = wallet_request_response_for_release_at(
        &operation,
        "profile:alpha",
        "wallet-account-alpha",
        "wallet-request:11111111111111111111111111111111",
        flow_now,
    );
    let buy = bind_buy(&mint, "profile:alpha", profile, &purchase_effect).unwrap();

    let rights_request_1 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[0].provisioned.node_public_key,
        &operation,
    )
    .unwrap();
    let rights_request_2 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[1].provisioned.node_public_key,
        &operation,
    )
    .unwrap();
    let rights_request_3 = RightsProviderRequestV1::new_evaluate(
        ordered_fixtures[2].provisioned.node_public_key,
        &operation,
    )
    .unwrap();
    let chain1 = ProcessChainEvidenceProvider::new(rights_request_1, true);
    let chain2 = ProcessChainEvidenceProvider::new(rights_request_2, true);
    let chain3 = ProcessChainEvidenceProvider::new(rights_request_3, true);
    ordered_fixtures[0]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain1.clone())
        .await
        .unwrap();
    ordered_fixtures[1]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain2.clone())
        .await
        .unwrap();
    ordered_fixtures[2]
        .registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain3.clone())
        .await
        .unwrap();

    let runtime_owner_parent = temp_root.join("runtime-owner-only-parent");
    owner_only_dir(&runtime_owner_parent);
    let runtime_data_dir = runtime_owner_parent.join("runtime-release");
    let coordinator = RuntimeReleaseCoordinator::new(
        RuntimeReleaseJournal::new(runtime_data_dir.clone()),
        runtime_operation_issuer_for_seed(0x21),
        vec![
            RuntimeSelectedProvider::new(
                ordered_fixtures[0].provisioned.node_public_key,
                &ordered_fixtures[0].adapter,
                &ordered_fixtures[0].adapter,
            ),
            RuntimeSelectedProvider::new(
                ordered_fixtures[1].provisioned.node_public_key,
                &ordered_fixtures[1].adapter,
                &ordered_fixtures[1].adapter,
            ),
            RuntimeSelectedProvider::new(
                ordered_fixtures[2].provisioned.node_public_key,
                &ordered_fixtures[2].adapter,
                &ordered_fixtures[2].adapter,
            ),
        ],
    )
    .unwrap()
    .with_response_clock(crate::auth::now_ts);
    let outcome = coordinator
        .release(
            &wallet_request,
            &wallet_response,
            operation.clone(),
            crate::auth::now_ts(),
        )
        .await
        .unwrap();
    let signed_node_contributions = match outcome {
        RuntimeReleaseCoordinatorOutcome::Terminal(
            RuntimeReleaseTerminalResult::ContributionsReady {
                signed_node_contributions,
            },
        ) => signed_node_contributions,
        other => panic!("unexpected coordinator outcome: {other:?}"),
    };
    assert_eq!(signed_node_contributions.len(), 2);
    assert_eq!(chain1.requests().await.len(), 1);
    assert_eq!(chain2.requests().await.len(), 1);
    assert_eq!(chain3.requests().await.len(), 0);
    let expected_terminal = RuntimeReleaseCoordinatorOutcome::Terminal(
        RuntimeReleaseTerminalResult::ContributionsReady {
            signed_node_contributions: signed_node_contributions.clone(),
        },
    );
    let replay_coordinator = RuntimeReleaseCoordinator::new(
        RuntimeReleaseJournal::new(runtime_data_dir.clone()),
        runtime_operation_issuer_for_seed(0x21),
        vec![
            RuntimeSelectedProvider::new(
                ordered_fixtures[0].provisioned.node_public_key,
                &ordered_fixtures[0].adapter,
                &ordered_fixtures[0].adapter,
            ),
            RuntimeSelectedProvider::new(
                ordered_fixtures[1].provisioned.node_public_key,
                &ordered_fixtures[1].adapter,
                &ordered_fixtures[1].adapter,
            ),
            RuntimeSelectedProvider::new(
                ordered_fixtures[2].provisioned.node_public_key,
                &ordered_fixtures[2].adapter,
                &ordered_fixtures[2].adapter,
            ),
        ],
    )
    .unwrap()
    .with_response_clock(crate::auth::now_ts);
    let replay_outcome = replay_coordinator
        .release(
            &wallet_request,
            &wallet_response,
            operation.clone(),
            crate::auth::now_ts(),
        )
        .await
        .unwrap();
    assert_eq!(replay_outcome, expected_terminal);
    assert_eq!(chain1.requests().await.len(), 1);
    assert_eq!(chain2.requests().await.len(), 1);
    assert_eq!(chain3.requests().await.len(), 0);
    assert!(list_unresolved_runtime_releases(&runtime_data_dir)
        .unwrap()
        .is_empty());
    assert!(unresolved_release_audit_records(&runtime_data_dir)
        .unwrap()
        .is_empty());

    let terminal_receipt = make_signed_terminal_receipt_at(
        &operation,
        &signed_node_contributions,
        0x61,
        crate::auth::now_ts(),
    );

    let wrong_open = open_viewer_session(
        &decrypt,
        &RuntimeOpenViewerSessionInput {
            buy: &buy,
            prepared_recipient: &prepared_b,
            signed_runtime_release_operation: &operation,
            expected_terminal_issuer: terminal_receipt.statement().issuer(),
            content_key_commitment: envelope.manifest().content_key_commitment(),
            media_identity: &protected_media_identity,
            protected_init_segment: &protected_init_segment,
            signed_node_contributions: &signed_node_contributions,
            signed_terminal_receipt: &terminal_receipt,
            now_unix_seconds: crate::auth::now_ts(),
        },
    )
    .await;
    assert_eq!(wrong_open, Err(RuntimeOpenError::MintSelection));
    cancel_prepared_recipient(&decrypt, &prepared_b)
        .await
        .unwrap();
    cancel_prepared_recipient(&decrypt, &prepared_b)
        .await
        .unwrap();
    let wrong_media_identity = media_identity(0x41);
    let wrong_object_open = open_viewer_session(
        &decrypt,
        &RuntimeOpenViewerSessionInput {
            buy: &buy,
            prepared_recipient: &prepared_a,
            signed_runtime_release_operation: &operation,
            expected_terminal_issuer: terminal_receipt.statement().issuer(),
            content_key_commitment: envelope.manifest().content_key_commitment(),
            media_identity: &wrong_media_identity,
            protected_init_segment: &protected_init_segment,
            signed_node_contributions: &signed_node_contributions,
            signed_terminal_receipt: &terminal_receipt,
            now_unix_seconds: crate::auth::now_ts(),
        },
    )
    .await;
    assert_eq!(wrong_object_open, Err(RuntimeOpenError::DecryptResult));

    let session = open_viewer_session(
        &decrypt,
        &RuntimeOpenViewerSessionInput {
            buy: &buy,
            prepared_recipient: &prepared_a,
            signed_runtime_release_operation: &operation,
            expected_terminal_issuer: terminal_receipt.statement().issuer(),
            content_key_commitment: envelope.manifest().content_key_commitment(),
            media_identity: &protected_media_identity,
            protected_init_segment: &protected_init_segment,
            signed_node_contributions: &signed_node_contributions,
            signed_terminal_receipt: &terminal_receipt,
            now_unix_seconds: crate::auth::now_ts(),
        },
    )
    .await
    .unwrap();

    let clear_init = read_viewer_media_part(
        &decrypt,
        &session,
        ViewerMediaPartSelectorV1::init(),
        crate::auth::now_ts(),
    )
    .await
    .unwrap();
    assert_eq!(clear_init.clear_media_part(), clear_init_segment.as_slice());

    let mut tampered_segment = encrypted_segments[1].clone();
    tampered_segment[0] ^= 0x01;
    let tampered_selector = ViewerMediaPartSelectorV1::segment(1, tampered_segment).unwrap();
    assert_eq!(
        read_viewer_media_part(&decrypt, &session, tampered_selector, crate::auth::now_ts(),).await,
        Err(RuntimeOpenError::DecryptResult)
    );

    let selector = ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap();
    let clear_segment =
        read_viewer_media_part(&decrypt, &session, selector.clone(), crate::auth::now_ts())
            .await
            .unwrap();
    let clear_segment_replay =
        read_viewer_media_part(&decrypt, &session, selector.clone(), crate::auth::now_ts())
            .await
            .unwrap();
    assert_eq!(clear_segment.part_selector(), &selector);
    assert_eq!(
        clear_segment.clear_media_part(),
        clear_segment_replay.clear_media_part()
    );
    assert_eq!(
        clear_segment.clear_media_part(),
        clear_segments[1].as_slice()
    );

    close_viewer_session(&decrypt, &session).await.unwrap();
    close_viewer_session(&decrypt, &session).await.unwrap();
    cancel_prepared_recipient(&decrypt, &prepared_a)
        .await
        .unwrap();

    let session_debug = format!("{session:?}");
    let init_debug = format!("{clear_init:?}");
    let segment_debug = format!("{clear_segment:?}");
    assert!(!session_debug.contains("carrier-provider-plane"));
    assert!(!session_debug.contains("node-1"));
    assert!(!session_debug.contains("node-2"));
    assert!(!session_debug.contains("node-3"));
    assert!(!init_debug.contains("share"));
    assert!(!segment_debug.contains("share"));
    assert!(!segment_debug.contains("cek"));

    assert!(!any_file_contains(&runtime_data_dir, &sealed_share_bytes));
    assert!(!any_file_contains(
        &runtime_data_dir,
        &protected_init_segment
    ));
    assert!(!any_file_contains(
        &runtime_data_dir,
        &encrypted_segments[0]
    ));
    assert!(!any_file_contains(&runtime_data_dir, &clear_init_segment));
    assert!(!any_file_contains(&runtime_data_dir, &clear_segments[0]));

    decrypt_registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();
    let mut schemes = decrypt_registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, Vec::<String>::new());
    protect_registry
        .unregister_runtime_provider_target(PROTECT_PROVIDER_ID)
        .await
        .unwrap();
    let mut protect_schemes = protect_registry.sub_provider_schemes().await;
    protect_schemes.sort();
    assert_eq!(protect_schemes, Vec::<String>::new());
    assert_eq!(
        prepare_recipient(
            &decrypt,
            &buy,
            RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
            RuntimeReleaseAuditIdV1::new(digest(0xaf)).unwrap(),
            runtime_operation_issuer_for_seed(0x21),
            crate::auth::now_ts(),
            crate::auth::now_ts() + 45,
        )
        .await,
        Err(RuntimeOpenError::DecryptResult)
    );

    assert!(list_unresolved_runtime_releases(&runtime_data_dir)
        .unwrap()
        .is_empty());
    assert!(unresolved_release_audit_records(&runtime_data_dir)
        .unwrap()
        .is_empty());

    for fixture in ordered_fixtures {
        fixture
            .registry
            .unregister_runtime_provider_target(CUSTODY_PROVIDER_ID)
            .await
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_registration_rejects_missing_or_unsafe_root_before_provider_use() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("missing-data");
    owner_only_dir(&data_dir);
    let (binary, pid_file, request_log) = write_mock_custody_provider(temp.path());
    let registry = Arc::new(ProviderRegistry::new());

    let error = register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("missing or unsafe"));
    assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
    assert!(!request_log.exists());
    assert!(!pid_file.exists());

    let unsafe_data_dir = temp.path().join("unsafe-data");
    owner_only_dir(&unsafe_data_dir);
    let unsafe_root = inactive_custody_state_root(&unsafe_data_dir);
    owner_only_dir(&unsafe_root);
    fs::set_permissions(&unsafe_root, fs::Permissions::from_mode(0o755)).unwrap();
    let error = register_inactive_custody_provider(&registry, &binary, &unsafe_data_dir)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("missing or unsafe"));
    assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
    assert!(!request_log.exists());
    assert!(!pid_file.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_registry_unregisters_shutdowns_and_restarts_bridge() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&inactive_custody_state_root(&data_dir));
    let (binary, pid_file, _request_log) = write_mock_custody_provider(temp.path());
    let registry = Arc::new(ProviderRegistry::new());

    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();
    let first_pid = read_pid(&pid_file);

    registry
        .unregister_runtime_provider_target(CUSTODY_PROVIDER_ID)
        .await
        .unwrap();
    assert!(!process_is_running(first_pid));

    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();
    let second_pid = read_pid(&pid_file);
    assert_ne!(first_pid, second_pid);
    let response = invoke_json_provider(
        registry.as_ref(),
        CUSTODY_PROVIDER_ID,
        "release_contribution",
        json!({"op": "release_contribution"}),
    )
    .await
    .unwrap();
    assert_eq!(response["echo"], "custody");

    registry
        .unregister_runtime_provider_target(CUSTODY_PROVIDER_ID)
        .await
        .unwrap();
    assert!(!process_is_running(second_pid));
}

#[cfg(unix)]
#[tokio::test]
async fn inactive_custody_duplicate_registration_rejects_and_settles_rejected_child() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    owner_only_dir(&inactive_custody_state_root(&data_dir));
    let (binary, pid_file, _request_log) = write_mock_custody_provider(temp.path());
    let registry = Arc::new(ProviderRegistry::new());

    register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap();
    let first_pid = read_pid(&pid_file);

    let error = register_inactive_custody_provider(&registry, &binary, &data_dir)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("failed to register inactive custody route"));

    let pids = read_pids(&pid_file);
    assert_eq!(pids.len(), 2);
    let rejected_pid = *pids.last().unwrap();
    assert_ne!(first_pid, rejected_pid);
    assert!(process_is_running(first_pid));
    assert!(!process_is_running(rejected_pid));

    registry
        .unregister_runtime_provider_target(CUSTODY_PROVIDER_ID)
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn protected_content_decrypt_startup_rejects_invalid_status_and_settles_child() {
    let issuer = runtime_operation_issuer_for_seed(0x41);
    let mut cases = Vec::new();
    let mut wrong_identity = protected_content_decrypt_provider_status();
    wrong_identity["data"]["provider"] = json!("decrypt");
    cases.push(("identity", wrong_identity));
    let mut wrong_request_schema = protected_content_decrypt_provider_status();
    wrong_request_schema["data"]["request_schema"] = json!("wrong-request-schema");
    cases.push(("request-schema", wrong_request_schema));
    let mut wrong_response_schema = protected_content_decrypt_provider_status();
    wrong_response_schema["data"]["response_schema"] = json!("wrong-response-schema");
    cases.push(("response-schema", wrong_response_schema));
    let mut wrong_version = protected_content_decrypt_provider_status();
    wrong_version["data"]["version"] = json!("wrong-version");
    cases.push(("version", wrong_version));
    let mut unconfigured = protected_content_decrypt_provider_status();
    unconfigured["data"]["configured"] = json!(false);
    cases.push(("configured", unconfigured));
    let mut wrong_operations = protected_content_decrypt_provider_status();
    wrong_operations["data"]["supported_operations"] = json!(["status", "shutdown"]);
    cases.push(("operations", wrong_operations));
    let mut extra_data = protected_content_decrypt_provider_status();
    extra_data["data"]["route"] = json!("private");
    cases.push(("extra-data", extra_data));
    let mut extra_top = protected_content_decrypt_provider_status();
    extra_top["route"] = json!("private");
    cases.push(("extra-top", extra_top));

    for (name, status) in cases {
        let temp = tempfile::tempdir().unwrap();
        let case_root = temp.path().join(name);
        fs::create_dir_all(&case_root).unwrap();
        let (binary, pid_file, request_log) =
            write_mock_protected_content_decrypt_provider(&case_root, issuer, &status);
        let registry = Arc::new(ProviderRegistry::new());

        register_protected_content_decrypt_provider(&registry, &binary, issuer)
            .await
            .expect_err("invalid protected-content decrypt status must fail closed");

        assert!(!process_is_running(read_pid(&pid_file)), "case {name}");
        let requests = fs::read_to_string(&request_log).unwrap();
        assert!(requests.contains(r#""op":"status""#), "case {name}");
        assert!(requests.contains(r#""op":"shutdown""#), "case {name}");
        assert!(matches!(
            registry
                .invoke_provider(ProviderInvocation {
                    source: RUNTIME_PROVIDER_ID.to_string(),
                    target: PROTECTED_CONTENT_DECRYPT_PROVIDER_ID.to_string(),
                    op: "status".to_string(),
                    request: json!({"op":"status"}),
                    transfer: ProviderTransfer::Json,
                    range: None,
                    progress: None,
                    transport: ProviderInvocationTransport::Local,
                })
                .await,
            Err(ProviderError::NoProvider(target))
                if target == PROTECTED_CONTENT_DECRYPT_PROVIDER_ID
        ));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn protected_content_decrypt_startup_rejects_wrong_issuer_and_settles_child() {
    let temp = tempfile::tempdir().unwrap();
    let expected_issuer = runtime_operation_issuer_for_seed(0x41);
    let supplied_issuer = runtime_operation_issuer_for_seed(0x42);
    let (binary, pid_file, request_log) = write_mock_protected_content_decrypt_provider(
        temp.path(),
        expected_issuer,
        &protected_content_decrypt_provider_status(),
    );
    let registry = Arc::new(ProviderRegistry::new());

    register_protected_content_decrypt_provider(&registry, &binary, supplied_issuer)
        .await
        .expect_err("a provider that rejects the Runtime issuer must fail closed");

    assert!(!process_is_running(read_pid(&pid_file)));
    let requests = fs::read_to_string(&request_log).unwrap();
    assert!(requests.contains(&format!(
        r#""trusted_runtime_issuer":"0x{}""#,
        hex::encode(supplied_issuer.as_bytes())
    )));
    assert!(requests.contains(r#""op":"shutdown""#));
    assert!(!requests.contains(r#""op":"status""#));
}

#[cfg(unix)]
#[tokio::test]
async fn protected_content_decrypt_duplicate_and_restart_settle_exact_processes() {
    let temp = tempfile::tempdir().unwrap();
    let issuer = runtime_operation_issuer_for_seed(0x41);
    let (binary, pid_file, request_log) = write_mock_protected_content_decrypt_provider(
        temp.path(),
        issuer,
        &protected_content_decrypt_provider_status(),
    );
    let registry = Arc::new(ProviderRegistry::new());

    register_protected_content_decrypt_provider(&registry, &binary, issuer)
        .await
        .unwrap();
    let first_pid = read_pid(&pid_file);
    assert!(process_is_running(first_pid));

    let duplicate = register_protected_content_decrypt_provider(&registry, &binary, issuer)
        .await
        .unwrap_err();
    assert!(duplicate.to_string().contains("already registered"));
    let duplicate_pids = read_pids(&pid_file);
    assert_eq!(duplicate_pids.len(), 2);
    let rejected_pid = duplicate_pids[1];
    assert!(process_is_running(first_pid));
    assert!(!process_is_running(rejected_pid));

    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();
    assert!(!process_is_running(first_pid));

    register_protected_content_decrypt_provider(&registry, &binary, issuer)
        .await
        .unwrap();
    let replacement_pid = read_pid(&pid_file);
    assert_ne!(replacement_pid, first_pid);
    assert_ne!(replacement_pid, rejected_pid);
    assert!(process_is_running(replacement_pid));
    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();
    assert!(!process_is_running(replacement_pid));

    let requests = fs::read_to_string(&request_log).unwrap();
    assert_eq!(requests.matches(r#""op":"shutdown""#).count(), 3);
}

#[tokio::test]
async fn runtime_rights_adapter_fails_closed_without_chain_and_does_not_call_rights() {
    let registry = Arc::new(ProviderRegistry::new());
    let rights = RecordingProvider::new(
        "rights",
        json!({"status": "ok", "data": {"echo": "rights"}}),
    );
    registry
        .register_sub_provider("rights", rights.clone())
        .await
        .unwrap();
    let err = invoke_json_provider(
        registry.as_ref(),
        CHAIN_PROVIDER_ID,
        CHAIN_RIGHTS_EVIDENCE_OP,
        json!({
            "op": CHAIN_RIGHTS_EVIDENCE_OP,
            "signed_runtime_release_operation": "0xab"
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("invocation failed"));
    assert!(rights.requests().await.is_empty());
    let mut schemes = registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, vec!["rights".to_string()]);
}

#[tokio::test]
async fn decrypt_registry_adapter_dispatches_prepare_recipient_exactly_once() {
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_prepared_recipient(
                    RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
                    opaque_handle(0x21),
                    recipient_public_key(0x30),
                    &recipient_identity(0x30),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    let drm = RecordingProvider::new("drm", json!({"status": "ok", "data": {"echo": "drm"}}));
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();
    registry
        .register_sub_provider("drm", drm.clone())
        .await
        .unwrap();
    let adapter = RuntimeDecryptRegistryAdapter::new(registry.clone());
    let request = decrypt_prepare_request();
    let expected_request = serde_json::to_value(&request).unwrap();

    let response = RuntimeDecryptProvider::prepare_recipient(&adapter, &request)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        DecryptProviderResponseStatusV1::PreparedRecipient
    );
    assert_eq!(
        response.audit_request_id().unwrap(),
        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap()
    );
    let requests = decrypt.requests().await;
    assert_eq!(requests.len(), 1);
    assert_exact_runtime_decrypt_invocation(&requests[0], "prepare_recipient", &expected_request);
    assert!(drm.requests().await.is_empty());
}

#[tokio::test]
async fn decrypt_registry_adapter_dispatches_open_viewer_session_exactly_once() {
    let registry = Arc::new(ProviderRegistry::new());
    let (request, audit_id, handle, _init_segment, _segments) = decrypt_open_fixture();
    let decrypt = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_session_opened(audit_id, handle).unwrap(),
            )
            .unwrap(),
        ),
    );
    let key = RecordingProvider::new("key", json!({"status": "ok", "data": {"echo": "key"}}));
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();
    registry
        .register_sub_provider("key", key.clone())
        .await
        .unwrap();
    let adapter = RuntimeDecryptRegistryAdapter::new(registry.clone());
    let expected_request = serde_json::to_value(&request).unwrap();

    let response = RuntimeDecryptProvider::open_viewer_session(&adapter, &request)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        DecryptProviderResponseStatusV1::ViewerSessionOpened
    );
    assert_eq!(response.audit_request_id().unwrap(), audit_id);
    assert_eq!(response.viewer_session_handle().unwrap(), &handle);
    let requests = decrypt.requests().await;
    assert_eq!(requests.len(), 1);
    assert_exact_runtime_decrypt_invocation(&requests[0], "open_viewer_session", &expected_request);
    assert!(key.requests().await.is_empty());
}

#[tokio::test]
async fn decrypt_registry_adapter_dispatches_read_cancel_and_close_exactly_once() {
    let registry = Arc::new(ProviderRegistry::new());
    let (_open_request, audit_id, handle, _init_segment, encrypted_segments) =
        decrypt_open_fixture();
    let read_request = DecryptProviderRequestV1::new_read_viewer_media_part(
        audit_id,
        handle,
        ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap(),
    )
    .unwrap();
    let cancel_request =
        DecryptProviderRequestV1::new_cancel_prepared_recipient(audit_id, handle).unwrap();
    let close_request =
        DecryptProviderRequestV1::new_close_viewer_session(audit_id, handle).unwrap();
    let adapter = RuntimeDecryptRegistryAdapter::new(registry.clone());

    let read_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_media_part(
                    audit_id,
                    handle,
                    ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap(),
                    vec![0x10, 0x11, 0x12],
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            read_provider.clone(),
        )
        .await
        .unwrap();
    let read_expected = serde_json::to_value(&read_request).unwrap();
    let read_response = RuntimeDecryptProvider::read_viewer_media_part(&adapter, &read_request)
        .await
        .unwrap();
    assert_eq!(
        read_response.status(),
        DecryptProviderResponseStatusV1::ViewerMediaPart
    );
    assert_eq!(read_provider.requests().await.len(), 1);
    assert_exact_runtime_decrypt_invocation(
        &read_provider.requests().await[0],
        "read_viewer_media_part",
        &read_expected,
    );
    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();

    let cancel_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_cancelled_prepared_recipient(audit_id, handle)
                    .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            cancel_provider.clone(),
        )
        .await
        .unwrap();
    let cancel_expected = serde_json::to_value(&cancel_request).unwrap();
    let cancel_response =
        RuntimeDecryptProvider::cancel_prepared_recipient(&adapter, &cancel_request)
            .await
            .unwrap();
    assert_eq!(
        cancel_response.status(),
        DecryptProviderResponseStatusV1::CancelledPreparedRecipient
    );
    assert_exact_runtime_decrypt_invocation(
        &cancel_provider.requests().await[0],
        "cancel_prepared_recipient",
        &cancel_expected,
    );
    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();

    let close_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_closed_viewer_session(audit_id, handle).unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            close_provider.clone(),
        )
        .await
        .unwrap();
    let close_expected = serde_json::to_value(&close_request).unwrap();
    let close_response = RuntimeDecryptProvider::close_viewer_session(&adapter, &close_request)
        .await
        .unwrap();
    assert_eq!(
        close_response.status(),
        DecryptProviderResponseStatusV1::ClosedViewerSession
    );
    assert_exact_runtime_decrypt_invocation(
        &close_provider.requests().await[0],
        "close_viewer_session",
        &close_expected,
    );
}

#[tokio::test]
async fn decrypt_registry_adapter_fails_closed_on_provider_status_and_data_errors() {
    let request = decrypt_prepare_request();

    for response in [
        json!({"status": "error", "code": "invalid_request"}),
        json!({"data": {}}),
        json!({"status": "weird", "data": {}}),
        json!({"status": "ok", "data": {"status": "prepared_recipient"}}),
    ] {
        let registry = Arc::new(ProviderRegistry::new());
        let decrypt = RecordingProvider::new("decrypt", response);
        registry
            .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt)
            .await
            .unwrap();
        let adapter = RuntimeDecryptRegistryAdapter::new(registry);
        assert_eq!(
            RuntimeDecryptProvider::prepare_recipient(&adapter, &request).await,
            Err(RuntimeProviderCallError::NoExactResult)
        );
    }
}

#[tokio::test]
async fn decrypt_registry_adapter_fails_closed_without_registered_decrypt_provider() {
    let registry = Arc::new(ProviderRegistry::new());
    let provisional =
        RecordingProvider::new("drm", json!({"status": "ok", "data": {"echo": "drm"}}));
    registry
        .register_sub_provider("drm", provisional.clone())
        .await
        .unwrap();
    let adapter = RuntimeDecryptRegistryAdapter::new(registry);
    let request = decrypt_prepare_request();
    assert_eq!(
        RuntimeDecryptProvider::prepare_recipient(&adapter, &request).await,
        Err(RuntimeProviderCallError::NoExactResult)
    );
    assert!(provisional.requests().await.is_empty());
}

#[test]
fn runtime_protected_content_id_is_exact_lowercase_domain_hash_and_changes_for_mutations() {
    let identity = EncryptedContentIdentityV1::new(digest(0x41), 2048).unwrap();
    let derived = runtime_protected_content_id(&identity).unwrap();
    let expected = format!(
        "content:{}",
        hex::encode(identity.canonical_hash().unwrap().as_bytes())
    );
    assert_eq!(derived, expected);
    assert!(derived.starts_with("content:"));
    assert_eq!(derived.len(), "content:".len() + 64);
    assert!(derived["content:".len()..]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')));

    let changed_digest = EncryptedContentIdentityV1::new(digest(0x42), 2048).unwrap();
    let changed_len = EncryptedContentIdentityV1::new(digest(0x41), 2049).unwrap();
    assert_ne!(
        runtime_protected_content_id(&changed_digest).unwrap(),
        derived
    );
    assert_ne!(runtime_protected_content_id(&changed_len).unwrap(), derived);
}

#[tokio::test]
async fn runtime_resolve_rights_policy_recomputes_identity_and_rejects_mismatch() {
    let encrypted_content_identity = EncryptedContentIdentityV1::new(digest(0x51), 4096).unwrap();
    let access_id = content_access_id(0x51);
    let policy = policy_body_for(
        encrypted_content_identity.clone(),
        access_id,
        RightsActionV1::View,
    );
    let registry = Arc::new(ProviderRegistry::new());
    let chain = RecordingProvider::new(
        CHAIN_PROVIDER_ID,
        ok_provider_response(json!({
            "schema": CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1,
            "policy_body": format!("0x{}", hex::encode(policy.canonical_bytes().unwrap())),
        })),
    );
    registry
        .register_sub_provider(CHAIN_PROVIDER_ID, chain.clone())
        .await
        .unwrap();
    let resolved = resolve_runtime_rights_policy(
        registry.as_ref(),
        &encrypted_content_identity,
        access_id,
        RightsActionV1::View,
    )
    .await
    .unwrap();
    assert_eq!(resolved.body(), &policy);
    assert_eq!(resolved.identity(), &policy.policy_identity().unwrap());

    let invocation = chain.requests().await;
    assert_eq!(invocation.len(), 1);
    assert_eq!(
        invocation[0]["_runtime_invocation"]["target"],
        Value::String(CHAIN_PROVIDER_ID.to_string())
    );
    assert_eq!(
        invocation[0]["op"],
        Value::String("resolve_protected_content_policy".to_string())
    );

    for mismatched_policy in [
        policy_body_for(encrypted_content(0x52), access_id, RightsActionV1::View),
        policy_body_for(
            encrypted_content_identity.clone(),
            content_access_id(0x52),
            RightsActionV1::View,
        ),
        policy_body_for(
            encrypted_content_identity.clone(),
            access_id,
            RightsActionV1::Download,
        ),
    ] {
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider(
                CHAIN_PROVIDER_ID,
                RecordingProvider::new(
                    CHAIN_PROVIDER_ID,
                    ok_provider_response(json!({
                        "schema": CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1,
                        "policy_body": format!(
                            "0x{}",
                            hex::encode(mismatched_policy.canonical_bytes().unwrap())
                        ),
                    })),
                ),
            )
            .await
            .unwrap();
        assert!(resolve_runtime_rights_policy(
            registry.as_ref(),
            &encrypted_content_identity,
            access_id,
            RightsActionV1::View
        )
        .await
        .is_err());
    }
}

#[cfg(unix)]
fn media_preparation_source_input(
    root: &Path,
    principal_id: &str,
) -> RuntimeCustodyLibrarySourceInput {
    RuntimeCustodyLibrarySourceInput {
        object_uri: "localhost://Users/test/Documents/source.mp4".to_string(),
        principal_id: principal_id.to_string(),
        source_file_path: root.join("source.mp4"),
        wallet_account_id: "wallet-account-1".to_string(),
        wallet_account_address: "0x1111111111111111111111111111111111111111".to_string(),
        creator_mint_source_digest: digest(0x71),
        copies: "0x1".to_string(),
        price: "0x5".to_string(),
        source_storage: "protected_principal_root".to_string(),
    }
}

#[cfg(unix)]
fn setup_media_preparation_root(temp: &Path) -> (PathBuf, PathBuf) {
    let data_dir = temp.join("data");
    owner_only_dir(&data_dir);
    let protected_root = data_dir.join("protected-content");
    owner_only_dir(&protected_root);
    let provider_root = protected_root.join("media-provider");
    owner_only_dir(&provider_root);
    let staging_root = provider_root.join("staging");
    owner_only_dir(&staging_root);
    let source_path = temp.join("source.mp4");
    fs::write(&source_path, b"bounded source media").unwrap();
    fs::set_permissions(&source_path, fs::Permissions::from_mode(0o600)).unwrap();
    (data_dir, staging_root)
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_media_preparation_reuses_exact_settled_output_and_rejects_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    let (data_dir, staging_root) = setup_media_preparation_root(temp.path());
    let input = media_preparation_source_input(temp.path(), "person:local:media-retry");
    let provider = Arc::new(TestMediaPreparationProvider {
        staging_root: staging_root.clone(),
        requests: Mutex::new(Vec::new()),
        response: TestMediaPreparationResponse::Prepared,
    });
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_runtime_provider_target(MEDIA_PROVIDER_ID, provider.clone())
        .await
        .unwrap();

    let first = prepare_runtime_custody_library_source(&data_dir, registry.as_ref(), &input)
        .await
        .expect("first preparation must settle");
    assert!(matches!(first, RuntimeLibraryMediaPreparation::Prepared(_)));
    let second = prepare_runtime_custody_library_source(&data_dir, registry.as_ref(), &input)
        .await
        .expect("exact retry must reuse prepared output");
    assert!(matches!(
        second,
        RuntimeLibraryMediaPreparation::Prepared(_)
    ));
    assert_eq!(provider.requests.lock().await.len(), 1);

    let operation_id = provider.requests.lock().await[0].clone();
    fs::remove_file(
        staging_root
            .join(operation_id)
            .join("prepared/segments/00000001.m4s"),
    )
    .unwrap();
    let corrupt = prepare_runtime_custody_library_source(&data_dir, registry.as_ref(), &input)
        .await
        .expect_err("corrupt settled output must fail closed without redispatch");
    assert!(corrupt.to_string().contains("output is invalid"));
    assert_eq!(provider.requests.lock().await.len(), 1);

    fs::write(&input.source_file_path, b"changed source media").unwrap();
    let error = prepare_runtime_custody_library_source(&data_dir, registry.as_ref(), &input)
        .await
        .expect_err("changed source must conflict with durable authority");
    assert!(error
        .to_string()
        .contains("conflicts with existing authority"));
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_media_preparation_unknown_settlement_never_redispatches() {
    let temp = tempfile::tempdir().unwrap();
    let (data_dir, staging_root) = setup_media_preparation_root(temp.path());
    let input = media_preparation_source_input(temp.path(), "person:local:media-unknown");
    let request_id = RuntimeMintIntent::request_id_for_source(
        &input.principal_id,
        &input.object_uri,
        &input.source_storage,
    )
    .unwrap();
    let provider = Arc::new(TestMediaPreparationProvider {
        staging_root,
        requests: Mutex::new(Vec::new()),
        response: TestMediaPreparationResponse::UnknownSettlement,
    });
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_runtime_provider_target(MEDIA_PROVIDER_ID, provider.clone())
        .await
        .unwrap();

    for _ in 0..2 {
        let error = prepare_runtime_custody_library_source(&data_dir, registry.as_ref(), &input)
            .await
            .expect_err("unknown settlement must require reconciliation");
        assert!(error.to_string().contains("settlement reconciliation"));
    }
    assert_eq!(provider.requests.lock().await.len(), 1);
    assert_eq!(
        runtime_mint_journal(&data_dir)
            .load_media_preparation(request_id)
            .unwrap()
            .state(),
        RuntimeMediaPreparationState::EffectPending
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_media_preparation_settled_failure_is_terminal_and_cleans_staging() {
    let temp = tempfile::tempdir().unwrap();
    let (data_dir, staging_root) = setup_media_preparation_root(temp.path());
    let input = media_preparation_source_input(temp.path(), "person:local:media-failed");
    let request_id = RuntimeMintIntent::request_id_for_source(
        &input.principal_id,
        &input.object_uri,
        &input.source_storage,
    )
    .unwrap();
    let provider = Arc::new(TestMediaPreparationProvider {
        staging_root: staging_root.clone(),
        requests: Mutex::new(Vec::new()),
        response: TestMediaPreparationResponse::SettledFailure,
    });
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_runtime_provider_target(MEDIA_PROVIDER_ID, provider.clone())
        .await
        .unwrap();

    for _ in 0..2 {
        let error = prepare_runtime_custody_library_source(&data_dir, registry.as_ref(), &input)
            .await
            .expect_err("settled provider failure must remain terminal");
        assert!(error.to_string().contains("media preparation failed"));
    }
    assert_eq!(provider.requests.lock().await.len(), 1);
    assert_eq!(
        runtime_mint_journal(&data_dir)
            .load_media_preparation(request_id)
            .unwrap()
            .state(),
        RuntimeMediaPreparationState::Failed
    );
    assert!(fs::read_dir(staging_root).unwrap().next().is_none());
}

#[tokio::test]
async fn runtime_custody_library_publish_fails_closed_without_composition() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let (clear_init_segment, clear_segments) = clear_media_components(0x41);
    let error = publish_runtime_custody_library_object(
        &data_dir,
        Arc::new(ProviderRegistry::new()),
        RuntimeCustodyLibraryPublishInput {
            object_uri: "localhost://Users/test/Documents/media".to_string(),
            principal_id: "person:local:runtime-custody-missing-composition".to_string(),
            mime_type: MEDIA_MIME_TYPE_V1.to_string(),
            codecs: MEDIA_CODECS_V1.to_string(),
            wallet_account_id: "wallet-account-1".to_string(),
            wallet_account_address: "0x1111111111111111111111111111111111111111".to_string(),
            creator_mint_source_digest: digest(0x71),
            copies: "0x1".to_string(),
            price: "0x5".to_string(),
            clear_init_segment,
            clear_segments,
            source_storage: "plain_localhost_root".to_string(),
        },
    )
    .await
    .expect_err("missing composition must fail closed");
    assert_eq!(
        error.to_string(),
        RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE
    );
    assert!(!data_dir.join("protected-content/runtime-mint").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_fails_closed_without_device_key() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    write_owner_only_custody_composition_config(
        &data_dir,
        &custody_composition_config(
            now,
            custody_route_bindings(
                &epoch,
                [
                    RuntimeCustodyRouteTransportConfig::Local,
                    RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                        peer_did: peer_did_for_seed(0xa1),
                    },
                    RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                        peer_did: peer_did_for_seed(0xa2),
                    },
                ],
            ),
        ),
    );
    let (clear_init_segment, clear_segments) = clear_media_components(0x41);
    let error = publish_runtime_custody_library_object(
        &data_dir,
        Arc::new(ProviderRegistry::new()),
        RuntimeCustodyLibraryPublishInput {
            object_uri: "localhost://Users/test/Documents/media".to_string(),
            principal_id: "person:local:runtime-custody-missing-device-key".to_string(),
            mime_type: MEDIA_MIME_TYPE_V1.to_string(),
            codecs: MEDIA_CODECS_V1.to_string(),
            wallet_account_id: "wallet-account-1".to_string(),
            wallet_account_address: "0x1111111111111111111111111111111111111111".to_string(),
            creator_mint_source_digest: digest(0x71),
            copies: "0x1".to_string(),
            price: "0x5".to_string(),
            clear_init_segment,
            clear_segments,
            source_storage: "plain_localhost_root".to_string(),
        },
    )
    .await
    .expect_err("missing device key must fail closed");
    assert!(
        error
            .to_string()
            .contains("local Runtime device signing key is missing"),
        "{error}"
    );
    assert!(!data_dir.join("protected-content/runtime-mint").exists());
}

#[cfg(unix)]
fn library_publish_test_routes(
    epoch: &SignedCustodyEpochV1,
) -> Vec<RuntimeCustodyRouteBindingConfig> {
    custody_route_bindings(
        epoch,
        [
            RuntimeCustodyRouteTransportConfig::Local,
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: peer_did_for_seed(0xa1),
            },
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: peer_did_for_seed(0xa2),
            },
        ],
    )
}

#[cfg(unix)]
fn library_publish_test_input(principal_id: &str) -> RuntimeCustodyLibraryPublishInput {
    let (clear_init_segment, clear_segments) = clear_media_components(0x41);
    RuntimeCustodyLibraryPublishInput {
        object_uri: "localhost://Users/test/Documents/media".to_string(),
        principal_id: principal_id.to_string(),
        mime_type: MEDIA_MIME_TYPE_V1.to_string(),
        codecs: MEDIA_CODECS_V1.to_string(),
        wallet_account_id: "wallet-account-1".to_string(),
        wallet_account_address: "0x1111111111111111111111111111111111111111".to_string(),
        creator_mint_source_digest: digest(0x71),
        copies: "0x1".to_string(),
        price: "0x5".to_string(),
        clear_init_segment,
        clear_segments,
        source_storage: "plain_localhost_root".to_string(),
    }
}

#[cfg(unix)]
fn library_publish_request_id(input: &RuntimeCustodyLibraryPublishInput) -> Digest32 {
    RuntimeMintIntent::request_id_for_source(
        &input.principal_id,
        &input.object_uri,
        &input.source_storage,
    )
    .unwrap()
}

#[cfg(unix)]
pub(crate) fn write_library_publish_test_composition(
    data_dir: &Path,
) -> (SignedCustodyEpochV1, u64) {
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    write_owner_only_custody_composition_config(
        data_dir,
        &custody_composition_config(now, library_publish_test_routes(&epoch)),
    );
    // Return the clock read used to derive the composition identities: the
    // custody-pool identity is content-addressed over clock-derived windows,
    // so any fixture that must produce the identical pool/committee identity
    // has to reuse this exact value instead of reading the clock again.
    (epoch, now)
}

#[cfg(unix)]
pub(crate) fn library_publish_test_mint_composition(
    data_dir: &Path,
) -> (
    Vec<RuntimeMintNodeBinding>,
    CustodyPoolIdentityV1,
    elastos_protected_content_contracts::CustodyEpochIdentityV1,
    CustodyCommitteeAuthorizationIdentityV1,
) {
    write_library_publish_test_composition(data_dir);
    let registry = Arc::new(ProviderRegistry::new());
    let composition = load_runtime_custody_composition(data_dir, registry)
        .unwrap()
        .unwrap();
    let configured = composition.configured_nodes().unwrap();
    let selected = resolve_runtime_mint_selected_nodes(
        composition.expected_policy_authority,
        composition.expected_authorization_identity,
        &composition.signed_pool,
        &composition.signed_epoch,
        &composition.signed_committee_authorization,
        crate::auth::now_ts(),
        &configured,
    )
    .unwrap();
    let bindings = selected.iter().map(|node| node.binding().clone()).collect();
    let pool = composition.signed_pool.pool_identity().unwrap();
    let epoch = composition.signed_epoch.epoch_identity().unwrap();
    let committee = composition
        .signed_committee_authorization
        .authorization_identity()
        .unwrap();
    (bindings, pool, epoch, committee)
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_fails_closed_without_protect_provider() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let _ = write_library_publish_test_composition(&data_dir);
    let input = library_publish_test_input("person:local:runtime-custody-missing-protect");
    let request_id = library_publish_request_id(&input);
    let error =
        publish_runtime_custody_library_object(&data_dir, Arc::new(ProviderRegistry::new()), input)
            .await
            .expect_err("missing protect provider must fail closed");
    assert!(
        error
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{error}"
    );
    let intent = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(intent.protect_state_label(), "not_started");
    assert!(!intent.provider_effect_started());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_fails_closed_without_chain_policy() {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let _ = write_library_publish_test_composition(&data_dir);
    let registry = Arc::new(ProviderRegistry::new());
    register_protect_provider(&registry, &protect_binary)
        .await
        .unwrap();
    let input = library_publish_test_input("person:local:runtime-custody-missing-policy");
    let request_id = library_publish_request_id(&input);
    let error = publish_runtime_custody_library_object(&data_dir, registry.clone(), input)
        .await
        .expect_err("missing chain policy must fail closed");
    assert!(
        error
            .to_string()
            .contains("Runtime custody rights policy is unavailable"),
        "{error}"
    );
    let intent = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert!(intent.protect_terminal_before_draft());
    assert_eq!(intent.protect_terminal_settlement_label(), Some("closed"));

    registry
        .unregister_runtime_provider_target(PROTECT_PROVIDER_ID)
        .await
        .unwrap();
    let replay = publish_runtime_custody_library_object(
        &data_dir,
        registry,
        library_publish_test_input("person:local:runtime-custody-missing-policy"),
    )
    .await
    .expect_err("settled-before-draft retry must remain terminal");
    assert!(
        replay
            .to_string()
            .contains(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE),
        "{replay}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_retries_exactly_when_protect_never_dispatches() {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let (epoch, _composition_now) = write_library_publish_test_composition(&data_dir);
    let registry = Arc::new(ProviderRegistry::new());
    let input = library_publish_test_input("person:local:runtime-custody-pre-dispatch-retry");
    let request_id = library_publish_request_id(&input);
    let first = publish_runtime_custody_library_object(&data_dir, registry.clone(), input)
        .await
        .expect_err("missing protect provider must fail closed");
    assert!(
        first
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{first}"
    );

    let first_intent = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(first_intent.protect_state_label(), "not_started");
    let expected_access_id = first_intent.content_access_id();

    register_protect_provider(&registry, &protect_binary)
        .await
        .unwrap();
    registry
        .register_sub_provider(
            CUSTODY_PROVIDER_ID,
            Arc::new(LibraryMintCustodyProvider {
                expected_issuer: derived_device_runtime_issuer(0x21),
                nodes: epoch
                    .statement()
                    .nodes()
                    .iter()
                    .map(|node| node.node_public_key())
                    .collect(),
            }),
        )
        .await
        .unwrap();
    registry
        .set_carrier_invoker(Arc::new(LoopbackCustodyCarrierInvoker {
            registry: Arc::downgrade(&registry),
        }))
        .await;
    registry
        .register_sub_provider(CHAIN_PROVIDER_ID, Arc::new(LibraryMintChainPolicyProvider))
        .await
        .unwrap();
    let (device_key, _) = derived_device_key_for_seed(0x21);
    let content = ContentAvailabilityTestProvider::with_signing_key(
        device_key,
        ContentAvailabilityTestConfig {
            checked_at: crate::auth::now_ts(),
            ..ContentAvailabilityTestConfig::accepted()
        },
    );
    registry
        .register_sub_provider("content", content)
        .await
        .unwrap();

    let published = publish_runtime_custody_library_object(
        &data_dir,
        registry.clone(),
        library_publish_test_input("person:local:runtime-custody-pre-dispatch-retry"),
    )
    .await
    .expect("exact retry must reuse the persisted intent");
    assert!(published.content_id.starts_with("content:"));

    let reloaded = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(reloaded.content_access_id(), expected_access_id);
    assert_eq!(reloaded.completed_mint_id(), Some(published.mint_id));

    registry
        .unregister_runtime_provider_target(PROTECT_PROVIDER_ID)
        .await
        .unwrap();
    registry
        .unregister_runtime_provider_target(CUSTODY_PROVIDER_ID)
        .await
        .unwrap();
    registry
        .unregister_sub_provider(CHAIN_PROVIDER_ID)
        .await
        .unwrap();
    registry.unregister_sub_provider("content").await.unwrap();

    let media_provider_root = data_dir.join("protected-content/media-provider");
    owner_only_dir(&media_provider_root);
    owner_only_dir(&media_provider_root.join("staging"));
    let source_file_path = temp.path().join("completed-source.mp4");
    fs::write(&source_file_path, b"completed source media").unwrap();
    fs::set_permissions(&source_file_path, fs::Permissions::from_mode(0o600)).unwrap();
    let source = RuntimeCustodyLibrarySourceInput {
        object_uri: "localhost://Users/test/Documents/media".to_string(),
        principal_id: "person:local:runtime-custody-pre-dispatch-retry".to_string(),
        source_file_path: source_file_path.clone(),
        wallet_account_id: "wallet-account-1".to_string(),
        wallet_account_address: "0x1111111111111111111111111111111111111111".to_string(),
        creator_mint_source_digest: digest(0x71),
        copies: "0x1".to_string(),
        price: "0x5".to_string(),
        source_storage: "plain_localhost_root".to_string(),
    };
    let preparation = RuntimeMediaPreparationRecord::new(
        &source.principal_id,
        &source.object_uri,
        &source.source_storage,
        source_media_digest(&source_file_path).unwrap(),
        MEDIA_PROVIDER_ID,
        source.wallet_account_id.clone(),
        source.wallet_account_address.clone(),
        source.creator_mint_source_digest,
    )
    .unwrap();
    let preparation_receipt = digest(0xd1);
    let journal = runtime_mint_journal(&data_dir);
    journal.persist_media_preparation(&preparation).unwrap();
    journal
        .mark_media_preparation_effect_started(preparation.request_id())
        .unwrap();
    journal
        .mark_media_preparation_prepared(preparation.request_id(), preparation_receipt)
        .unwrap();
    journal
        .mark_media_preparation_consumed(
            preparation.request_id(),
            preparation_receipt,
            published.mint_id,
        )
        .unwrap();
    let (source_replay, source_replay_input) =
        publish_runtime_custody_library_source(&data_dir, registry.clone(), source)
            .await
            .expect("consumed media preparation must replay without provider dispatch");
    assert_eq!(source_replay.mint_id, published.mint_id);
    assert!(source_replay_input.clear_init_segment.is_empty());
    assert!(source_replay_input.clear_segments.is_empty());

    let replay = publish_runtime_custody_library_object(
        &data_dir,
        registry,
        library_publish_test_input("person:local:runtime-custody-pre-dispatch-retry"),
    )
    .await
    .expect("completed mint replay must remain idempotent without providers");
    assert_eq!(replay.content_cid, published.content_cid);
    assert_eq!(replay.content_id, published.content_id);
    assert_eq!(replay.mint_id, published.mint_id);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_rejects_creator_account_switch_before_protect_effects() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    write_library_publish_test_composition(&data_dir);
    let registry = Arc::new(ProviderRegistry::new());
    let input = library_publish_test_input("person:local:runtime-custody-creator-account-switch");
    let request_id = library_publish_request_id(&input);
    let first = publish_runtime_custody_library_object(&data_dir, registry.clone(), input.clone())
        .await
        .expect_err("missing protect provider must fail closed");
    assert!(
        first
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{first}"
    );

    let mut switched = input;
    switched.wallet_account_id = "wallet-account-2".to_string();
    switched.wallet_account_address = "0x2222222222222222222222222222222222222222".to_string();
    let second = publish_runtime_custody_library_object(&data_dir, registry, switched)
        .await
        .expect_err("creator account drift must fail closed");
    assert!(
        second
            .to_string()
            .contains("Runtime custody mint intent conflicts with existing authority"),
        "{second}"
    );

    let intent = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(intent.creator_wallet_account_id(), "wallet-account-1");
    assert_eq!(
        intent.creator_wallet_address(),
        "0x1111111111111111111111111111111111111111"
    );
    assert_eq!(intent.protect_state_label(), "not_started");
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_blocks_exact_retry_after_ambiguous_protect_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let _ = write_library_publish_test_composition(&data_dir);
    let protect = SequencedProvider::new(
        PROTECT_PROVIDER_ID,
        vec![
            Err(ProviderError::Provider(
                "simulated protect bridge failure".to_string(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_opened(
                    [0x51; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
                    b"placeholder-protected-init",
                )
                .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_cancelled(
                    [0x51; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
                )
                .unwrap(),
            )),
        ],
    );
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(protect.clone()).await;
    let input = library_publish_test_input("person:local:runtime-custody-ambiguous-dispatch");
    let request_id = library_publish_request_id(&input);
    let first = publish_runtime_custody_library_object(&data_dir, registry.clone(), input)
        .await
        .expect_err("ambiguous protect dispatch must fail closed");
    assert!(
        first
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{first}"
    );
    let pending = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(pending.protect_state_label(), "open_request_pending");

    let second = publish_runtime_custody_library_object(
        &data_dir,
        registry.clone(),
        library_publish_test_input("person:local:runtime-custody-ambiguous-dispatch"),
    )
    .await
    .expect_err("exact retry must recover and settle the ambiguous open");
    assert!(
        second
            .to_string()
            .contains(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE),
        "{second}"
    );
    let settled = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert!(settled.protect_terminal_before_draft());
    assert_eq!(
        settled.protect_terminal_settlement_label(),
        Some("cancelled")
    );
    let requests = protect.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["op"], "open_protection_session");
    assert_eq!(requests[1]["op"], "open_protection_session");
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[2]["op"], "cancel_protection_session");
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_rejects_creator_source_drift_during_open_request_pending()
{
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    write_library_publish_test_composition(&data_dir);
    let protect = SequencedProvider::new(
        PROTECT_PROVIDER_ID,
        vec![Err(ProviderError::Provider(
            "simulated protect bridge failure".to_string(),
        ))],
    );
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(protect.clone()).await;
    let input = library_publish_test_input("person:local:runtime-custody-source-drift");
    let request_id = library_publish_request_id(&input);
    let first = publish_runtime_custody_library_object(&data_dir, registry.clone(), input.clone())
        .await
        .expect_err("ambiguous protect dispatch must fail closed");
    assert!(
        first
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{first}"
    );
    let pending = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(pending.protect_state_label(), "open_request_pending");

    let mut drifted = input;
    drifted.creator_mint_source_digest = digest(0x72);
    let second = publish_runtime_custody_library_object(&data_dir, registry, drifted)
        .await
        .expect_err("creator source drift must fail closed");
    assert!(
        second
            .to_string()
            .contains("Runtime custody mint intent conflicts with existing authority"),
        "{second}"
    );

    let reloaded = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(reloaded.protect_state_label(), "open_request_pending");
    let requests = protect.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["op"], "open_protection_session");
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_settles_cancelled_session_before_failing() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let _ = write_library_publish_test_composition(&data_dir);
    let handle = [0x31; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
    let protect = SequencedProvider::new(
        PROTECT_PROVIDER_ID,
        vec![
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_opened(handle, b"placeholder-protected-init")
                    .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_failure(ProviderFailureCodeV1::BackendUnavailable)
                    .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_cancelled(handle).unwrap(),
            )),
        ],
    );
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(protect.clone()).await;
    let input = library_publish_test_input("person:local:runtime-custody-cancel-settlement");
    let request_id = library_publish_request_id(&input);
    let first = publish_runtime_custody_library_object(&data_dir, registry.clone(), input)
        .await
        .expect_err("protect segment failure must fail closed");
    assert!(
        first
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{first}"
    );
    let settled = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert!(settled.protect_terminal_before_draft());
    assert_eq!(
        settled.protect_terminal_settlement_label(),
        Some("cancelled")
    );

    let second = publish_runtime_custody_library_object(
        &data_dir,
        registry,
        library_publish_test_input("person:local:runtime-custody-cancel-settlement"),
    )
    .await
    .expect_err("acted intent must not redispatch after cancel settlement");
    assert!(
        second
            .to_string()
            .contains(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE),
        "{second}"
    );
    let requests = protect.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2]["op"], "cancel_protection_session");
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_recovers_open_handle_by_cancelling_before_redispatch() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let _ = write_library_publish_test_composition(&data_dir);
    let registry = Arc::new(ProviderRegistry::new());
    let input = library_publish_test_input("person:local:runtime-custody-open-handle-recovery");
    let request_id = library_publish_request_id(&input);
    let composition = load_runtime_custody_composition(&data_dir, registry.clone())
        .unwrap()
        .unwrap();
    let configured = composition.configured_nodes().unwrap();
    let selected = resolve_runtime_mint_selected_nodes(
        composition.expected_policy_authority,
        composition.expected_authorization_identity,
        &composition.signed_pool,
        &composition.signed_epoch,
        &composition.signed_committee_authorization,
        crate::auth::now_ts(),
        &configured,
    )
    .unwrap();
    let mint_nodes = selected
        .iter()
        .map(|node| node.binding().clone())
        .collect::<Vec<_>>();
    let journal = runtime_mint_journal(&data_dir);
    load_or_persist_runtime_mint_intent(&journal, &composition, &input, mint_nodes).unwrap();
    journal
        .mark_intent_protect_effect_started(request_id)
        .unwrap();
    let handle = [0x61; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
    journal
        .mark_intent_protect_opened(request_id, handle)
        .unwrap();

    let protect = SequencedProvider::new(
        PROTECT_PROVIDER_ID,
        vec![Ok(ok_typed_protect_provider_response(
            ProtectProviderResponseV1::new_cancelled(handle).unwrap(),
        ))],
    );
    registry.register(protect.clone()).await;

    let error = publish_runtime_custody_library_object(&data_dir, registry, input)
        .await
        .expect_err("open-handle recovery must settle by cancelling");
    assert!(
        error
            .to_string()
            .contains(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE),
        "{error}"
    );
    let intent = journal.load_intent(request_id).unwrap();
    assert!(intent.protect_terminal_before_draft());
    assert_eq!(
        intent.protect_terminal_settlement_label(),
        Some("cancelled")
    );
    let requests = protect.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["op"], "cancel_protection_session");
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_retains_cleanup_obligation_when_close_fails() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let (epoch, composition_now) = write_library_publish_test_composition(&data_dir);
    let registry = Arc::new(ProviderRegistry::new());
    let input = library_publish_test_input("person:local:runtime-custody-close-failure");
    let request_id = library_publish_request_id(&input);
    let composition = load_runtime_custody_composition(&data_dir, registry.clone())
        .unwrap()
        .unwrap();
    let configured = composition.configured_nodes().unwrap();
    let selected = resolve_runtime_mint_selected_nodes(
        composition.expected_policy_authority,
        composition.expected_authorization_identity,
        &composition.signed_pool,
        &composition.signed_epoch,
        &composition.signed_committee_authorization,
        crate::auth::now_ts(),
        &configured,
    )
    .unwrap();
    let mint_nodes = selected
        .iter()
        .map(|node| node.binding().clone())
        .collect::<Vec<_>>();
    let mint_intent = load_or_persist_runtime_mint_intent(
        &runtime_mint_journal(&data_dir),
        &composition,
        &input,
        mint_nodes,
    )
    .unwrap();
    let clear_layout =
        ValidatedClearFmp4MediaSessionLayoutV1::new(&input.clear_init_segment).unwrap();
    let (_, protected_segments) = media_components(0x41);
    let protected_init = clear_layout
        .rewrite_protected_init(
            &input.clear_init_segment,
            *mint_intent.content_access_id().as_bytes(),
        )
        .unwrap();
    let protected_media = CencFmp4MediaIdentityV1::new_from_bytes(
        &protected_init,
        &protected_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
    )
    .unwrap();
    let committee = validated_custody_committee_for_epoch(&epoch, composition_now);
    let content_key =
        elastos_protected_content_custody::ContentEncryptionKeyV1::generate().unwrap();
    let envelope = provision_custody_envelope(
        protected_media.encrypted_content().clone(),
        &content_key,
        &committee,
    )
    .unwrap();
    let handle = [0x41; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
    let protect = SequencedProvider::new(
        PROTECT_PROVIDER_ID,
        vec![
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_opened(handle, &protected_init).unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_segment_protected(handle, 0, &protected_segments[0])
                    .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_segment_protected(handle, 1, &protected_segments[1])
                    .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_finalized(handle, &protected_media, &envelope)
                    .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_failure(ProviderFailureCodeV1::BackendUnavailable)
                    .unwrap(),
            )),
            Ok(ok_typed_protect_provider_response(
                ProtectProviderResponseV1::new_already_absent(handle).unwrap(),
            )),
        ],
    );
    registry.register(protect.clone()).await;

    let first = publish_runtime_custody_library_object(&data_dir, registry.clone(), input)
        .await
        .expect_err("close failure must retain cleanup ownership");
    assert!(
        first
            .to_string()
            .contains(RUNTIME_CUSTODY_MINT_RECONCILIATION_REQUIRED_MESSAGE),
        "{first}"
    );
    let pending = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert_eq!(
        pending.protect_state_label(),
        "open_handle_pending_close",
        "the close-failure scenario must reach the pending-close obligation; a \
         different label means the fixture was steered into another failure \
         mode: {pending:?}"
    );
    assert_eq!(
        pending.protect_pending_close_handle(),
        Some(handle),
        "{pending:?}"
    );

    let second = publish_runtime_custody_library_object(
        &data_dir,
        registry,
        library_publish_test_input("person:local:runtime-custody-close-failure"),
    )
    .await
    .expect_err("close replay loss must settle via already_absent");
    assert!(
        second
            .to_string()
            .contains(RUNTIME_CUSTODY_MINT_TERMINAL_ABORT_MESSAGE),
        "{second}"
    );
    let settled = runtime_mint_journal(&data_dir)
        .load_intent(request_id)
        .unwrap();
    assert!(settled.protect_terminal_before_draft());
    assert_eq!(
        settled.protect_terminal_settlement_label(),
        Some("already_absent")
    );
    let requests = protect.requests().await;
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[4]["op"], "close_protection_session");
    assert_eq!(requests[5]["op"], "close_protection_session");
}

struct LibraryMintCustodyProvider {
    expected_issuer: RuntimeOperationIssuerKeyV1,
    nodes: Vec<NodePublicKey>,
}

#[async_trait::async_trait]
impl Provider for LibraryMintCustodyProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "library mint custody provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![CUSTODY_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        CUSTODY_PROVIDER_ID
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let mut request = request.clone();
        if let Some(object) = request.as_object_mut() {
            object.remove("_runtime_invocation");
        }
        if request.get("op").and_then(Value::as_str) != Some("provision_node_share") {
            return Err(ProviderError::Provider(
                "library mint custody provider expected provision_node_share".to_string(),
            ));
        }
        let bytes = serde_json::to_vec(&request)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let now = crate::auth::now_ts();
        for node in &self.nodes {
            let Ok(validated) = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
                &bytes,
                self.expected_issuer,
                *node,
                now,
            ) else {
                continue;
            };
            let provision = validated.provision_node_share().map_err(|_| {
                ProviderError::Provider("library mint provision request is invalid".to_string())
            })?;
            let response = CustodyProviderResponseV1::new_provisioned(provision).map_err(|_| {
                ProviderError::Provider("library mint provision response is invalid".to_string())
            })?;
            let response = serde_json::from_slice(
                &response
                    .to_json_vec()
                    .map_err(|error| ProviderError::Provider(error.to_string()))?,
            )
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
            return Ok(ok_provider_response(response));
        }
        Err(ProviderError::Provider(
            "library mint custody provision did not match a selected node".to_string(),
        ))
    }
}

struct LibraryMintChainPolicyProvider;

#[async_trait::async_trait]
impl Provider for LibraryMintChainPolicyProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "library mint chain policy provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![CHAIN_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        CHAIN_PROVIDER_ID
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let encrypted_content = request
            .get("encrypted_content")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing encrypted_content".to_string()))
            .and_then(|value| {
                let hex = value.strip_prefix("0x").ok_or_else(|| {
                    ProviderError::Provider("invalid encrypted_content".to_string())
                })?;
                let bytes = hex::decode(hex).map_err(|_| {
                    ProviderError::Provider("invalid encrypted_content".to_string())
                })?;
                EncryptedContentIdentityV1::from_canonical_bytes(&bytes)
                    .map_err(|_| ProviderError::Provider("invalid encrypted_content".to_string()))
            })?;
        let content_access_id = request
            .get("content_access_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing content_access_id".to_string()))
            .and_then(|value| {
                let hex = value.strip_prefix("0x").ok_or_else(|| {
                    ProviderError::Provider("invalid content_access_id".to_string())
                })?;
                let bytes: [u8; 16] = hex::decode(hex)
                    .map_err(|_| ProviderError::Provider("invalid content_access_id".to_string()))?
                    .try_into()
                    .map_err(|_| {
                        ProviderError::Provider("invalid content_access_id".to_string())
                    })?;
                ContentAccessIdV1::new(bytes)
                    .map_err(|_| ProviderError::Provider("invalid content_access_id".to_string()))
            })?;
        if request.get("action").and_then(Value::as_str) != Some("view") {
            return Err(ProviderError::Provider(
                "unexpected policy action".to_string(),
            ));
        }
        let policy = policy_body_for(encrypted_content, content_access_id, RightsActionV1::View);
        Ok(ok_provider_response(json!({
            "schema": CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1,
            "policy_body": format!("0x{}", hex::encode(policy.canonical_bytes().unwrap())),
        })))
    }
}

struct LoopbackCustodyCarrierInvoker {
    registry: Weak<ProviderRegistry>,
}

#[async_trait::async_trait]
impl ProviderCarrierInvoker for LoopbackCustodyCarrierInvoker {
    async fn invoke_carrier_provider(
        &self,
        route: &ProviderCarrierRoute,
        invocation: &ProviderInvocation,
        mut request: Value,
    ) -> Result<Value, ProviderError> {
        if !matches!(route, ProviderCarrierRoute::PeerDid { .. }) {
            return Err(ProviderError::Provider(
                "library custody loopback requires an endpoint DID route".to_string(),
            ));
        }
        let runtime = request
            .get_mut("_runtime_invocation")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                ProviderError::Provider(
                    "library custody loopback requires a Runtime invocation envelope".to_string(),
                )
            })?;
        runtime.insert(
            "carrier".to_string(),
            json!({"source_endpoint_did": peer_did_for_seed(0xa0)}),
        );
        let registry = self.registry.upgrade().ok_or_else(|| {
            ProviderError::Provider("library mint carrier loopback registry is gone".to_string())
        })?;
        registry
            .send_runtime_provider_target_raw(&invocation.target, &request)
            .await
    }
}

struct AllowingProcessChainEvidenceProvider;

#[async_trait::async_trait]
impl Provider for AllowingProcessChainEvidenceProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "allowing process chain evidence provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![CHAIN_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        CHAIN_PROVIDER_ID
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        if request["_runtime_invocation"]["source"]
            != Value::String(RUNTIME_PROVIDER_ID.to_string())
            || request["_runtime_invocation"]["target"]
                != Value::String(CHAIN_PROVIDER_ID.to_string())
            || request["_runtime_invocation"]["op"]
                != Value::String(CHAIN_RIGHTS_EVIDENCE_OP.to_string())
            || request["_runtime_invocation"]["transport"]
                != Value::String("runtime-local-provider-plane".to_string())
            || request["_runtime_invocation"]["carrier"] != Value::Null
        {
            return Err(ProviderError::Provider(
                "chain evidence runtime envelope did not match the expected invocation".to_string(),
            ));
        }
        let mut inner_request = request.clone();
        let Some(inner_object) = inner_request.as_object_mut() else {
            return Err(ProviderError::Provider(
                "chain evidence request was not an object".to_string(),
            ));
        };
        inner_object.remove("_runtime_invocation");
        let signed_hex = inner_request
            .get("signed_runtime_release_operation")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Provider(
                    "chain evidence request is missing the signed release operation".to_string(),
                )
            })?;
        let signed_bytes = hex::decode(signed_hex.strip_prefix("0x").unwrap_or(signed_hex))
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let operation = SignedRuntimeReleaseOperationV1::from_canonical_bytes(&signed_bytes)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let selected = operation
            .statement()
            .custody_epoch()
            .statement()
            .nodes()
            .first()
            .ok_or_else(|| {
                ProviderError::Provider("signed release operation has no custody nodes".to_string())
            })?
            .node_public_key();
        let expected = RightsProviderRequestV1::new_evaluate(selected, &operation)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let evidence_now = crate::auth::now_ts();
        Ok(ok_provider_response(chain_evidence_for_request_at(
            &expected,
            evidence_now,
            evidence_now,
            true,
        )))
    }
}

struct LibraryProcessCustodyDispatcher {
    expected_issuer: RuntimeOperationIssuerKeyV1,
    nodes: Vec<(NodePublicKey, Arc<ProviderRegistry>)>,
}

impl LibraryProcessCustodyDispatcher {
    fn strip_envelope(request: &Value) -> Result<Value, ProviderError> {
        let mut inner = request.clone();
        if let Some(object) = inner.as_object_mut() {
            object.remove("_runtime_invocation");
        }
        Ok(inner)
    }

    fn node_registry(&self, request: &Value) -> Result<Arc<ProviderRegistry>, ProviderError> {
        let inner = Self::strip_envelope(request)?;
        let op = inner
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let bytes = serde_json::to_vec(&inner)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let now = crate::auth::now_ts();
        match op.as_str() {
            "status" => self
                .nodes
                .first()
                .map(|(_, registry)| registry.clone())
                .ok_or_else(|| {
                    ProviderError::Provider(
                        "library process custody dispatcher has no nodes".to_string(),
                    )
                }),
            "evaluate" => {
                let validated = ValidatedRightsProviderRequestV1::decode_and_validate_at(
                    &bytes,
                    self.expected_issuer,
                    now,
                )
                .map_err(|error| ProviderError::Provider(error.to_string()))?;
                self.nodes
                    .iter()
                    .find(|(node, _)| *node == validated.selected_node_public_key())
                    .map(|(_, registry)| registry.clone())
                    .ok_or_else(|| {
                        ProviderError::Provider(
                            "library process custody evaluate did not match a selected node"
                                .to_string(),
                        )
                    })
            }
            "provision_node_share" | "release_contribution" => {
                for (node, registry) in &self.nodes {
                    if ValidatedCustodyProviderRequestV1::decode_and_validate_at(
                        &bytes,
                        self.expected_issuer,
                        *node,
                        now,
                    )
                    .is_ok()
                    {
                        return Ok(registry.clone());
                    }
                }
                Err(ProviderError::Provider(
                    "library process custody request did not match a selected node".to_string(),
                ))
            }
            _ => Err(ProviderError::Provider(format!(
                "library process custody dispatcher rejected op {op}"
            ))),
        }
    }
}

#[async_trait::async_trait]
impl Provider for LibraryProcessCustodyDispatcher {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "library process custody dispatcher is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![CUSTODY_PROVIDER_ID]
    }

    fn name(&self) -> &'static str {
        CUSTODY_PROVIDER_ID
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let registry = self.node_registry(request)?;
        registry
            .send_runtime_provider_target_raw(CUSTODY_PROVIDER_ID, request)
            .await
    }
}

struct LibraryReleaseWalletProvider;

#[async_trait::async_trait]
impl Provider for LibraryReleaseWalletProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "library release wallet provider is invoke-only".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["wallet"]
    }

    fn name(&self) -> &'static str {
        "wallet"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let inner = request.get("request").cloned().ok_or_else(|| {
            ProviderError::Provider("wallet request is missing the v2 envelope".to_string())
        })?;
        let bytes = serde_json::to_vec(&inner)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let now = crate::auth::now_ts();
        let wallet_request = WalletProviderRequestV2::decode_at(&bytes, now)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let (account_id, canonical_rights_request_hex) = match &wallet_request.operation {
            WalletProviderOperationV2::RequestProtectedContentRightsSignature {
                account_id,
                canonical_rights_request_hex,
                ..
            } => (account_id.clone(), canonical_rights_request_hex.clone()),
            _ => {
                return Err(ProviderError::Provider(
                    "library release wallet expected a protected-content rights signature"
                        .to_string(),
                ));
            }
        };
        let rights_bytes = hex::decode(&canonical_rights_request_hex)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let rights_request = RightsRequestV1::from_canonical_bytes(&rights_bytes)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let key = WalletSigningKey::from_slice(&[7; 32])
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let (signature, recovery_id) = key
            .sign_prehash_recoverable(&elastos_auth::ethereum_signed_message_hash(&rights_bytes))
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let mut signature_bytes = signature.to_bytes().to_vec();
        signature_bytes.push(recovery_id.to_byte());
        let signed = WalletSignedRightsRequestV1::new(rights_request, signature_bytes)
            .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let result = ProtectedContentRightsSignatureResultV1::new(
            account_id,
            wallet_address_hex(wallet(7)),
            hex::encode(signed.canonical_bytes().unwrap()),
        )
        .map_err(|error| ProviderError::Provider(error.to_string()))?;
        let wallet_response = WalletProviderResponseV2::for_request(
            &wallet_request,
            WalletResultV2::Ok {
                data: serde_json::to_value(result)
                    .map_err(|error| ProviderError::Provider(error.to_string()))?,
            },
        );
        Ok(ok_provider_response(
            serde_json::to_value(wallet_response)
                .map_err(|error| ProviderError::Provider(error.to_string()))?,
        ))
    }
}

#[cfg(unix)]
pub(crate) fn write_device_key(data_dir: &Path, seed: u8) {
    let identity = data_dir.join("identity");
    fs::create_dir_all(&identity).unwrap();
    let path = identity.join("device.key");
    fs::write(&path, [seed; 32]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(unix)]
fn derived_device_key_for_seed(seed: u8) -> (SigningKey, String) {
    elastos_identity::derive_did(&[seed; 32])
}

#[cfg(unix)]
fn derived_device_runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
    let (key, _) = derived_device_key_for_seed(seed);
    RuntimeOperationIssuerKeyV1::new(key.verifying_key().to_bytes()).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_purchase_requires_current_verified_profile_authority() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-profile-check";
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let listing_path = super::runtime_listing_path(&harness.data_dir, mint.draft().mint_id());
    let listing_before = fs::read(&listing_path).unwrap();
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &derived_device_key_for_seed(0x27).1,
        crate::auth::now_ts(),
    );
    let purchase_path =
        super::runtime_purchase_path(&harness.data_dir, principal_id, mint.draft().mint_id());
    let purchase_json = String::from_utf8(fs::read(&purchase_path).unwrap()).unwrap();
    assert!(!purchase_json.contains("wallet_request_hex"));
    assert!(!purchase_json.contains("wallet_response_hex"));
    assert_eq!(fs::read(&listing_path).unwrap(), listing_before);
    let missing = super::load_runtime_custody_profile_did(&harness.data_dir, principal_id);
    assert!(missing.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_purchase_reconstruction_rejects_mismatched_profile_did() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-profile-check";
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let (_proof_binding_id, _profile_identity) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &derived_device_key_for_seed(0x27).1,
        crate::auth::now_ts(),
    );
    let mismatch = super::reconstructed_buy_receipt(&mint, &purchase, &current_profile_did);
    assert!(mismatch.is_err());
    assert!(mismatch
        .unwrap_err()
        .to_string()
        .contains("Runtime custody chain evidence is invalid"));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_release_wallet_uses_fresh_binding_per_session() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-profile-check";
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let (_proof_binding_id, _profile_identity) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let buy = super::reconstructed_buy_receipt(&mint, &purchase, &current_profile_did).unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register_sub_provider("wallet", Arc::new(LibraryReleaseWalletProvider))
        .await
        .unwrap();
    let session_a = super::derive_runtime_custody_session_binding(
        principal_id,
        &current_profile_did,
        TEST_VIEWER_LAUNCH_ID,
        "proof:alpha",
        "runtime-session:alpha",
        "grant:alpha",
        mint.draft().mint_id(),
    )
    .unwrap();
    let session_b = super::derive_runtime_custody_session_binding(
        principal_id,
        &current_profile_did,
        TEST_VIEWER_LAUNCH_ID,
        "proof:alpha",
        "runtime-session:beta",
        "grant:alpha",
        mint.draft().mint_id(),
    )
    .unwrap();
    let launch_b = super::derive_runtime_custody_session_binding(
        principal_id,
        &current_profile_did,
        TEST_VIEWER_LAUNCH_ID_B,
        "proof:alpha",
        "runtime-session:alpha",
        "grant:alpha",
        mint.draft().mint_id(),
    )
    .unwrap();
    assert_ne!(session_a, session_b);
    assert_ne!(session_a, launch_b);
    let now = crate::auth::now_ts();
    let (request_a, response_a, signed_a) = super::invoke_runtime_release_wallet(
        registry.as_ref(),
        &buy,
        &recipient_identity(0x30),
        super::RuntimeReleaseWalletInvocation {
            principal_id,
            account_id: &purchase.account_id,
            proof_binding_id: "proof:alpha",
            session_id: "runtime-session:alpha",
            grant_id: "grant:alpha",
            mint_id: mint.draft().mint_id(),
            runtime_session_binding: session_a,
        },
        now,
    )
    .await
    .unwrap();
    let expected_a = buy.binding_for_session(session_a).unwrap();
    assert_eq!(signed_a.request().binding(), &expected_a);
    let decoded_request_a = WalletProviderRequestV2::decode_at(&request_a, now).unwrap();
    WalletProviderResponseV2::decode_for_request(&response_a, &decoded_request_a).unwrap();
    let request_a_hex = match &decoded_request_a.operation {
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            canonical_rights_request_hex,
            ..
        } => canonical_rights_request_hex.clone(),
        other => panic!("unexpected wallet request: {other:?}"),
    };
    let request_a_binding =
        RightsRequestV1::from_canonical_bytes(&hex::decode(request_a_hex).unwrap()).unwrap();
    assert_eq!(request_a_binding.binding(), &expected_a);

    let (request_b, _response_b, signed_b) = super::invoke_runtime_release_wallet(
        registry.as_ref(),
        &buy,
        &recipient_identity(0x30),
        super::RuntimeReleaseWalletInvocation {
            principal_id,
            account_id: &purchase.account_id,
            proof_binding_id: "proof:alpha",
            session_id: "runtime-session:beta",
            grant_id: "grant:alpha",
            mint_id: mint.draft().mint_id(),
            runtime_session_binding: session_b,
        },
        now,
    )
    .await
    .unwrap();
    let expected_b = buy.binding_for_session(session_b).unwrap();
    assert_eq!(signed_b.request().binding(), &expected_b);
    assert_ne!(expected_a, expected_b);
    let decoded_request_b = WalletProviderRequestV2::decode_at(&request_b, now).unwrap();
    let request_b_hex = match &decoded_request_b.operation {
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            canonical_rights_request_hex,
            ..
        } => canonical_rights_request_hex.clone(),
        other => panic!("unexpected wallet request: {other:?}"),
    };
    let request_b_binding =
        RightsRequestV1::from_canonical_bytes(&hex::decode(request_b_hex).unwrap()).unwrap();
    assert_eq!(request_b_binding.binding(), &expected_b);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_open_replays_exact_active_session_without_provider_effects() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-replay";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x21),
        crate::auth::now_ts() + 60,
    );
    let open = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:alpha".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        open["viewer_session_handle"].as_str().unwrap(),
        hex::encode(session.viewer_session_handle())
    );
    assert_eq!(open["expires_at"].as_u64(), Some(session.expires_at()));
    assert_eq!(open["mime_type"], MEDIA_MIME_TYPE_V1);
    assert_eq!(open["codecs"], MEDIA_CODECS_V1);
    assert_eq!(open["has_init_segment"], true);
    assert_eq!(
        open["segment_count"].as_u64(),
        Some(u64::try_from(mint.draft().media_identity().encrypted_segments().len()).unwrap())
    );
    let open_keys = open
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        open_keys,
        std::collections::BTreeSet::from([
            "codecs",
            "expires_at",
            "has_init_segment",
            "mime_type",
            "mint_id",
            "schema",
            "segment_count",
            "viewer_session_handle",
        ])
    );
    assert!(harness.content_provider.requests().await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_open_rejects_substituted_session_without_provider_effects() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-replay";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x21),
        crate::auth::now_ts() + 60,
    );
    let err = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:beta".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();
    assert!(err
        .to_string()
        .contains(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE));
    assert!(harness.content_provider.requests().await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_open_rejects_missing_live_availability_before_new_effects() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig {
            live_multi_peer_proof: None,
            ..ContentAvailabilityTestConfig::accepted_now()
        },
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-missing-live-availability";
    write_device_key(&harness.data_dir, 0x21);
    let (_epoch, _composition_now) = write_library_publish_test_composition(&harness.data_dir);
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &profile_did,
        crate::auth::now_ts(),
    );
    let decrypt = PrepareOnlyCleanupDecryptProvider::new();
    harness
        .registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();
    let wallet = RecordingProvider::new("wallet", ok_provider_response(json!({})));
    harness
        .registry
        .register_sub_provider("wallet", wallet.clone())
        .await
        .unwrap();

    let error = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:alpha".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("Runtime custody content availability is unavailable"));
    assert!(super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .is_none());
    assert!(decrypt.requests().await.is_empty());
    assert!(wallet.requests().await.is_empty());
    assert!(harness
        .content_provider
        .requests()
        .await
        .iter()
        .any(|request| request.get("op").and_then(Value::as_str) == Some("status")));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_read_close_and_replay_settle_exactly() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-read-close";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x21),
        crate::auth::now_ts() + 60,
    );
    let registry = Arc::new(ProviderRegistry::new());
    let read_request = DecryptProviderRequestV1::new_read_viewer_media_part(
        RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
        *session.viewer_session_handle(),
        ViewerMediaPartSelectorV1::init(),
    )
    .unwrap();
    let read_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_media_part(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                    ViewerMediaPartSelectorV1::init(),
                    vec![0x10, 0x11, 0x12],
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            read_provider.clone(),
        )
        .await
        .unwrap();
    let early_segment = super::read_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
        Some(0),
    )
    .await
    .unwrap_err();
    assert_eq!(
        early_segment.to_string(),
        "Runtime custody viewer media part is invalid"
    );
    assert!(read_provider.requests().await.is_empty());
    let read = super::read_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(read["data"].as_str().unwrap())
            .unwrap(),
        vec![0x10, 0x11, 0x12]
    );
    assert_eq!(read_provider.requests().await.len(), 1);
    assert_exact_runtime_decrypt_invocation(
        &read_provider.requests().await[0],
        "read_viewer_media_part",
        &serde_json::to_value(&read_request).unwrap(),
    );
    let repeated_init = super::read_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        repeated_init.to_string(),
        "Runtime custody viewer media part is invalid"
    );
    assert_eq!(read_provider.requests().await.len(), 1);

    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();
    let close_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_closed_viewer_session(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            close_provider.clone(),
        )
        .await
        .unwrap();
    let close = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
    )
    .await
    .unwrap();
    assert_eq!(close["close_result"], "closed");
    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::Closed
    );
    let closed_record_bytes = fs::read(super::runtime_viewer_path(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    ))
    .unwrap();
    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();

    let replay = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
    )
    .await
    .unwrap();
    assert_eq!(replay["close_result"], "already_absent");
    let record_bytes = fs::read(super::runtime_viewer_path(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    ))
    .unwrap();
    assert_eq!(record_bytes, closed_record_bytes);
    let replay_record: super::RuntimeCustodyViewerRecord =
        serde_json::from_slice(&record_bytes).unwrap();
    assert_eq!(
        replay_record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::Closed
    );
    let record_json = String::from_utf8(record_bytes).unwrap();
    assert!(!record_json.contains("proof:alpha"));
    assert!(!record_json.contains("runtime-session:alpha"));
    assert!(!record_json.contains("grant:alpha"));
    assert!(!record_json.contains("http://"));
    assert!(!record_json.contains("encrypted_content_base64"));
    assert!(!record_json.contains("\"action\""));
    assert!(!record_json.contains("\"clear_media_part\""));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_close_rejects_wrong_principal_without_provider_effects() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-close-owner";
    let wrong_principal_id = "person:local:runtime-custody-viewer-close-other";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x29),
        crate::auth::now_ts() + 60,
    );
    let record_path =
        super::runtime_viewer_path(&harness.data_dir, principal_id, mint.draft().mint_id());
    let before = fs::read(&record_path).unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(serde_json::json!({"schema":"unused"})),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            decrypt_provider.clone(),
        )
        .await
        .unwrap();

    let error = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry,
        wrong_principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
    )
    .await
    .expect_err("expected wrong-principal close rejection");
    assert!(error
        .to_string()
        .contains("Runtime custody viewer session is unavailable"));
    assert!(decrypt_provider.requests().await.is_empty());
    assert_eq!(fs::read(&record_path).unwrap(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_close_retains_cleanup_pending_until_exact_settlement() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-close-reconcile";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x21),
        crate::auth::now_ts() + 60,
    );
    let registry = Arc::new(ProviderRegistry::new());
    let pending_provider = SequencedProvider::new(
        "decrypt",
        vec![Err(ProviderError::Provider("timeout".to_string()))],
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            pending_provider.clone(),
        )
        .await
        .unwrap();
    let err = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
    )
    .await
    .unwrap_err();
    assert!(err
        .to_string()
        .contains(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE));
    let pending = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        pending.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::CleanupPending
    );

    registry
        .unregister_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID)
        .await
        .unwrap();
    let absent_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_session_already_absent(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, absent_provider)
        .await
        .unwrap();
    let closed = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
    )
    .await
    .unwrap();
    assert_eq!(closed["close_result"], "already_absent");
    let settled = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        settled.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_open_pending_reconciliation_settles_no_dispatch_with_exact_close_and_cancel(
) {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-pending-no-dispatch";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let record = persist_runtime_custody_open_pending_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x51),
        crate::auth::now_ts() + 60,
    );
    let audit_request_id = RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap();
    let close_request =
        DecryptProviderRequestV1::new_close_viewer_session(audit_request_id, opaque_handle(0x51))
            .unwrap();
    let cancel_request = DecryptProviderRequestV1::new_cancel_prepared_recipient(
        audit_request_id,
        opaque_handle(0x51),
    )
    .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt = SequencedProvider::new(
        "decrypt",
        vec![
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_viewer_session_already_absent(
                        audit_request_id,
                        opaque_handle(0x51),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                        audit_request_id,
                        opaque_handle(0x51),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
        ],
    );
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();

    let settled = super::settle_runtime_custody_viewer_cleanup(
        &harness.data_dir,
        registry,
        principal_id,
        mint.draft().mint_id(),
        record,
    )
    .await
    .unwrap();

    assert_eq!(
        settled.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
    let requests = decrypt.requests().await;
    assert_eq!(requests.len(), 2);
    assert_exact_runtime_decrypt_invocation(
        &requests[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    assert_exact_runtime_decrypt_invocation(
        &requests[1],
        "cancel_prepared_recipient",
        &serde_json::to_value(&cancel_request).unwrap(),
    );
    let persisted = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        persisted.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
    assert!(harness.content_provider.requests().await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_open_pending_reconciliation_settles_response_loss_with_exact_close_and_cancel(
) {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-pending-response-loss";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let record = persist_runtime_custody_open_pending_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x52),
        crate::auth::now_ts() + 60,
    );
    let audit_request_id = RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap();
    let close_request =
        DecryptProviderRequestV1::new_close_viewer_session(audit_request_id, opaque_handle(0x52))
            .unwrap();
    let cancel_request = DecryptProviderRequestV1::new_cancel_prepared_recipient(
        audit_request_id,
        opaque_handle(0x52),
    )
    .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt = SequencedProvider::new(
        "decrypt",
        vec![
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_closed_viewer_session(
                        audit_request_id,
                        opaque_handle(0x52),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_prepared_recipient_already_absent(
                        audit_request_id,
                        opaque_handle(0x52),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
        ],
    );
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();

    let settled = super::settle_runtime_custody_viewer_cleanup(
        &harness.data_dir,
        registry,
        principal_id,
        mint.draft().mint_id(),
        record,
    )
    .await
    .unwrap();

    assert_eq!(
        settled.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::Closed
    );
    let requests = decrypt.requests().await;
    assert_eq!(requests.len(), 2);
    assert_exact_runtime_decrypt_invocation(
        &requests[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    assert_exact_runtime_decrypt_invocation(
        &requests[1],
        "cancel_prepared_recipient",
        &serde_json::to_value(&cancel_request).unwrap(),
    );
    let persisted = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        persisted.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::Closed
    );
    assert!(harness.content_provider.requests().await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_open_pending_survives_active_write_failure_and_failed_cleanup() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-pending-write-failure";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let open_pending = persist_runtime_custody_open_pending_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x53),
        crate::auth::now_ts() + 60,
    );
    let before = fs::read(super::runtime_viewer_path(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    ))
    .unwrap();
    let active = super::RuntimeCustodyViewerRecord::from_active_session(
        principal_id,
        &current_profile_did,
        mint.draft().mint_id(),
        &purchase.content_id,
        super::derive_runtime_custody_session_binding(
            principal_id,
            &current_profile_did,
            TEST_VIEWER_LAUNCH_ID,
            &proof_binding_id,
            "runtime-session:alpha",
            "grant:alpha",
            mint.draft().mint_id(),
        )
        .unwrap(),
        &RuntimeViewerSession::from_persisted_parts(
            digest(0x91),
            opaque_handle(0x53),
            mint.draft().encrypted_content().clone(),
            RightsActionV1::View,
            crate::auth::now_ts() + 60,
        )
        .unwrap(),
        crate::auth::now_ts(),
    )
    .unwrap();
    let viewers_dir =
        super::runtime_viewer_path(&harness.data_dir, principal_id, mint.draft().mint_id())
            .parent()
            .unwrap()
            .to_path_buf();
    fs::set_permissions(&viewers_dir, fs::Permissions::from_mode(0o500)).unwrap();
    let persist_error = super::persist_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
        &active,
    )
    .expect_err("expected active viewer persist failure");
    let _ = persist_error;
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt = SequencedProvider::new(
        "decrypt",
        vec![Err(ProviderError::Provider("timeout".to_string()))],
    );
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt)
        .await
        .unwrap();
    let cleanup_error = super::settle_runtime_custody_viewer_cleanup(
        &harness.data_dir,
        registry,
        principal_id,
        mint.draft().mint_id(),
        open_pending,
    )
    .await
    .err()
    .expect("expected cleanup failure");
    fs::set_permissions(&viewers_dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(cleanup_error
        .to_string()
        .contains(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE));
    assert_eq!(
        fs::read(super::runtime_viewer_path(
            &harness.data_dir,
            principal_id,
            mint.draft().mint_id(),
        ))
        .unwrap(),
        before
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_expiry_reconciles_exact_cleanup_before_read() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-expiry";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x21),
        crate::auth::now_ts().saturating_sub(1),
    );
    let registry = Arc::new(ProviderRegistry::new());
    let close_request = DecryptProviderRequestV1::new_close_viewer_session(
        RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
        *session.viewer_session_handle(),
    )
    .unwrap();
    let close_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_session_already_absent(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            close_provider.clone(),
        )
        .await
        .unwrap();
    let err = super::read_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some("grant:alpha"),
        None,
    )
    .await
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("Runtime custody viewer session is unavailable"));
    assert_eq!(close_provider.requests().await.len(), 1);
    assert_exact_runtime_decrypt_invocation(
        &close_provider.requests().await[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
    assert!(harness.content_provider.requests().await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_restart_reconciliation_settles_old_active_record_before_replay() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-restart-reconcile";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x31),
        crate::auth::now_ts() + 60,
    );
    let registry = Arc::new(ProviderRegistry::new());
    let close_request = DecryptProviderRequestV1::new_close_viewer_session(
        RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
        *session.viewer_session_handle(),
    )
    .unwrap();
    let close_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_session_already_absent(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            close_provider.clone(),
        )
        .await
        .unwrap();

    super::reconcile_runtime_custody_viewers_after_decrypt_registration(
        &harness.data_dir,
        registry.clone(),
    )
    .await
    .unwrap();

    assert_eq!(close_provider.requests().await.len(), 1);
    assert_exact_runtime_decrypt_invocation(
        &close_provider.requests().await[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_read_and_close_reject_substituted_session_without_provider_effects()
{
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-substituted-binding";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x2a),
        crate::auth::now_ts() + 60,
    );
    let record_path =
        super::runtime_viewer_path(&harness.data_dir, principal_id, mint.draft().mint_id());
    let before = fs::read(&record_path).unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(serde_json::json!({"schema":"unused"})),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            decrypt_provider.clone(),
        )
        .await
        .unwrap();

    let read_error = super::read_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:beta"),
        Some("grant:alpha"),
        None,
    )
    .await
    .unwrap_err();
    assert!(read_error
        .to_string()
        .contains("Runtime custody viewer session is unavailable"));

    let close_error = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry,
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:beta"),
        Some("grant:alpha"),
    )
    .await
    .unwrap_err();
    assert!(close_error
        .to_string()
        .contains("Runtime custody viewer session is unavailable"));
    assert!(decrypt_provider.requests().await.is_empty());
    assert_eq!(fs::read(&record_path).unwrap(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_read_and_close_reject_malformed_binding_without_provider_effects() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-malformed-binding";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x2b),
        crate::auth::now_ts() + 60,
    );
    let record_path =
        super::runtime_viewer_path(&harness.data_dir, principal_id, mint.draft().mint_id());
    let before = fs::read(&record_path).unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(serde_json::json!({"schema":"unused"})),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            decrypt_provider.clone(),
        )
        .await
        .unwrap();

    let read_error = super::read_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some(""),
        Some("grant:alpha"),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        read_error.to_string(),
        "Runtime custody viewer session is unavailable"
    );

    let close_error = super::close_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        principal_id,
        &hex::encode(harness.mint_id.as_bytes()),
        &hex::encode(session.viewer_session_handle()),
        Some(TEST_VIEWER_LAUNCH_ID),
        Some(&proof_binding_id),
        Some("runtime-session:alpha"),
        Some(""),
    )
    .await
    .unwrap_err();
    assert_eq!(
        close_error.to_string(),
        "Runtime custody viewer session is unavailable"
    );

    assert!(decrypt_provider.requests().await.is_empty());
    assert_eq!(fs::read(&record_path).unwrap(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_restart_reconciliation_settles_open_pending_record() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-restart-open-pending";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    persist_runtime_custody_open_pending_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x33),
        crate::auth::now_ts() + 60,
    );
    let audit_request_id = RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap();
    let close_request =
        DecryptProviderRequestV1::new_close_viewer_session(audit_request_id, opaque_handle(0x33))
            .unwrap();
    let cancel_request = DecryptProviderRequestV1::new_cancel_prepared_recipient(
        audit_request_id,
        opaque_handle(0x33),
    )
    .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt = SequencedProvider::new(
        "decrypt",
        vec![
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_viewer_session_already_absent(
                        audit_request_id,
                        opaque_handle(0x33),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                        audit_request_id,
                        opaque_handle(0x33),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
        ],
    );
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();

    super::reconcile_runtime_custody_viewers_after_decrypt_registration(
        &harness.data_dir,
        registry,
    )
    .await
    .unwrap();

    let requests = decrypt.requests().await;
    assert_eq!(requests.len(), 2);
    assert_exact_runtime_decrypt_invocation(
        &requests[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    assert_exact_runtime_decrypt_invocation(
        &requests[1],
        "cancel_prepared_recipient",
        &serde_json::to_value(&cancel_request).unwrap(),
    );
    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_restart_reconciliation_settles_cleanup_pending_record() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-restart-cleanup-pending";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x34),
        crate::auth::now_ts() + 60,
    );
    let mut record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    record.mark_cleanup_pending(crate::auth::now_ts());
    super::persist_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
        &record,
    )
    .unwrap();
    let close_request = DecryptProviderRequestV1::new_close_viewer_session(
        RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
        *session.viewer_session_handle(),
    )
    .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let close_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_session_already_absent(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            close_provider.clone(),
        )
        .await
        .unwrap();

    super::reconcile_runtime_custody_viewers_after_decrypt_registration(
        &harness.data_dir,
        registry,
    )
    .await
    .unwrap();

    assert_eq!(close_provider.requests().await.len(), 1);
    assert_exact_runtime_decrypt_invocation(
        &close_provider.requests().await[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    let settled = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        settled.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_expired_old_binding_allows_cleanup_before_fresh_open() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted_now(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-expired-old-binding";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    let session = persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x35),
        crate::auth::now_ts().saturating_sub(1),
    );
    let close_request = DecryptProviderRequestV1::new_close_viewer_session(
        RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
        *session.viewer_session_handle(),
    )
    .unwrap();
    let registry = Arc::new(ProviderRegistry::new());
    let close_provider = RecordingProvider::new(
        "decrypt",
        ok_provider_response(
            serde_json::to_value(
                DecryptProviderResponseV1::new_viewer_session_already_absent(
                    RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
                    *session.viewer_session_handle(),
                )
                .unwrap(),
            )
            .unwrap(),
        ),
    );
    registry
        .register_runtime_provider_target(
            PROTECTED_CONTENT_DECRYPT_PROVIDER_ID,
            close_provider.clone(),
        )
        .await
        .unwrap();
    registry
        .register_sub_provider("content", harness.content_provider.clone())
        .await
        .unwrap();

    let error = super::open_runtime_custody_viewer(
        &harness.data_dir,
        registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:beta".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains(RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE));
    assert_eq!(close_provider.requests().await.len(), 1);
    assert_exact_runtime_decrypt_invocation(
        &close_provider.requests().await[0],
        "close_viewer_session",
        &serde_json::to_value(&close_request).unwrap(),
    );
    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_reconciliation_ignores_terminal_histories_beyond_unresolved_limit()
{
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let profile_did = derived_device_key_for_seed(0x41).1;
    for index in 0..300usize {
        let principal_id = format!("person:local:runtime-custody-viewer-terminal-{index}");
        let purchase = persist_runtime_custody_purchase_for_mint(
            &harness.data_dir,
            &mint,
            &principal_id,
            &profile_did,
            crate::auth::now_ts(),
        );
        let session = RuntimeViewerSession::from_persisted_parts(
            Digest32::new([u8::try_from((index % 250) + 1).unwrap(); 32]),
            [u8::try_from((index % 250) + 1).unwrap(); MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
            mint.draft().encrypted_content().clone(),
            RightsActionV1::View,
            crate::auth::now_ts() + 60,
        )
        .unwrap();
        let mut record = super::RuntimeCustodyViewerRecord::from_active_session(
            &principal_id,
            &profile_did,
            mint.draft().mint_id(),
            &purchase.content_id,
            RuntimeSessionBindingV1::new(Digest32::new(
                [u8::try_from((index % 250) + 1).unwrap(); 32],
            ))
            .unwrap(),
            &session,
            crate::auth::now_ts(),
        )
        .unwrap();
        record.mark_terminal(
            elastos_protected_content_runtime::RuntimeViewerSessionCloseResult::Closed,
            crate::auth::now_ts(),
        );
        super::persist_runtime_custody_viewer_record(
            &harness.data_dir,
            &principal_id,
            mint.draft().mint_id(),
            &record,
        )
        .unwrap();
    }
    let principal_id = "person:local:runtime-custody-viewer-unresolved";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &load_profile_did_for_test(&harness.data_dir, principal_id),
        crate::auth::now_ts(),
    );
    persist_runtime_custody_open_pending_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x36),
        crate::auth::now_ts() + 60,
    );
    let registry = Arc::new(ProviderRegistry::new());
    let decrypt = SequencedProvider::new(
        "decrypt",
        vec![
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_viewer_session_already_absent(
                        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
                        opaque_handle(0x36),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
            Ok(ok_provider_response(
                serde_json::to_value(
                    DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
                        opaque_handle(0x36),
                    )
                    .unwrap(),
                )
                .unwrap(),
            )),
        ],
    );
    registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt)
        .await
        .unwrap();

    super::reconcile_runtime_custody_viewers_after_decrypt_registration(
        &harness.data_dir,
        registry,
    )
    .await
    .unwrap();

    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_restart_reconciliation_rejects_invalid_active_record() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-viewer-restart-invalid";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let current_profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let purchase = persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &current_profile_did,
        crate::auth::now_ts(),
    );
    persist_runtime_custody_active_viewer_for_purchase(
        &harness.data_dir,
        &mint,
        &purchase,
        &proof_binding_id,
        "runtime-session:alpha",
        "grant:alpha",
        opaque_handle(0x32),
        crate::auth::now_ts() + 60,
    );
    let mut record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .unwrap();
    record.viewer_session_handle = hex::encode([0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]);
    super::persist_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
        &record,
    )
    .unwrap();

    let error = super::reconcile_runtime_custody_viewers_after_decrypt_registration(
        &harness.data_dir,
        Arc::new(ProviderRegistry::new()),
    )
    .await
    .expect_err("expected invalid active record rejection");
    assert!(error.to_string().contains("invalid"));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_viewer_lifecycle_guard_serializes_same_key_and_leaves_other_keys_independent(
) {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    let first_entered = Arc::new(Notify::new());
    let same_key_attempted = Arc::new(Notify::new());
    let other_key_entered = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let same_key_acquired = Arc::new(AtomicBool::new(false));
    let other_key_acquired = Arc::new(AtomicBool::new(false));

    let first_entered_task = first_entered.clone();
    let release_first_task = release_first.clone();
    let first_data_dir = data_dir.clone();
    let first = tokio::spawn(async move {
        let _guard = super::acquire_runtime_custody_viewer_lifecycle_guard(
            &first_data_dir,
            "principal-a",
            digest(0x41),
        )
        .await;
        first_entered_task.notify_one();
        release_first_task.notified().await;
    });

    first_entered.notified().await;

    let same_key_flag = same_key_acquired.clone();
    let same_key_attempted_task = same_key_attempted.clone();
    let same_key_data_dir = data_dir.clone();
    let same_key = tokio::spawn(async move {
        same_key_attempted_task.notify_one();
        let _guard = super::acquire_runtime_custody_viewer_lifecycle_guard(
            &same_key_data_dir,
            "principal-a",
            digest(0x41),
        )
        .await;
        same_key_flag.store(true, Ordering::SeqCst);
    });

    let other_key_flag = other_key_acquired.clone();
    let other_key_entered_task = other_key_entered.clone();
    let other_key_data_dir = data_dir.clone();
    let other_key = tokio::spawn(async move {
        let _guard = super::acquire_runtime_custody_viewer_lifecycle_guard(
            &other_key_data_dir,
            "principal-b",
            digest(0x42),
        )
        .await;
        other_key_flag.store(true, Ordering::SeqCst);
        other_key_entered_task.notify_one();
    });

    same_key_attempted.notified().await;
    other_key_entered.notified().await;
    assert!(!same_key_acquired.load(Ordering::SeqCst));
    assert!(other_key_acquired.load(Ordering::SeqCst));

    release_first.notify_one();
    first.await.unwrap();
    same_key.await.unwrap();
    other_key.await.unwrap();
    assert!(same_key_acquired.load(Ordering::SeqCst));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_purchase_fails_closed_without_profile() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-missing-profile";
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &derived_device_key_for_seed(0x27).1,
        crate::auth::now_ts(),
    );
    let open = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some("proof:alpha".to_string()),
            session_id: Some("runtime-session:alpha".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        open.to_string()
            .contains(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE),
        "{open}"
    );
    assert!(harness.content_provider.requests().await.is_empty());
    assert!(super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id()
    )
    .unwrap()
    .is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_purchase_rejects_mismatched_purchased_profile() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-profile-mismatch";
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let other_principal = "person:local:runtime-custody-open-profile-other";
    let (_other_proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, other_principal);
    let other_profile_did = load_profile_did_for_test(&harness.data_dir, other_principal);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &other_profile_did,
        crate::auth::now_ts(),
    );
    let open = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:alpha".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        open.to_string()
            .contains("Runtime custody chain evidence is invalid"),
        "{open}"
    );
    assert!(harness.content_provider.requests().await.is_empty());
    assert!(super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id()
    )
    .unwrap()
    .is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_fails_closed_without_decrypt() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted_now(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-no-decrypt";
    write_device_key(&harness.data_dir, 0x21);
    let (_epoch, _composition_now) = write_library_publish_test_composition(&harness.data_dir);
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &profile_did,
        crate::auth::now_ts(),
    );
    let open = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:alpha".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        open.to_string()
            .contains(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE),
        "{open}"
    );
    assert!(super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id()
    )
    .unwrap()
    .is_none());
}

#[cfg(unix)]
fn process_custody_play_routes(
    epoch: &SignedCustodyEpochV1,
    owner_state_roots: [Digest32; 3],
) -> Vec<RuntimeCustodyRouteBindingConfig> {
    epoch
        .statement()
        .nodes()
        .iter()
        .zip(owner_state_roots)
        .zip([
            RuntimeCustodyRouteTransportConfig::Local,
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: peer_did_for_seed(0xa1),
            },
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: peer_did_for_seed(0xa2),
            },
        ])
        .map(
            |((node, owner_state_root), transport)| RuntimeCustodyRouteBindingConfig {
                node_public_key_base64: raw_b64_32(*node.node_public_key().as_bytes()),
                owner_state_root_base64: raw_b64_32(*owner_state_root.as_bytes()),
                transport,
            },
        )
        .collect()
}

#[cfg(unix)]
async fn register_test_decrypt_provider(
    registry: &Arc<ProviderRegistry>,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
) {
    let decrypt_binary = required_test_binary_path(TEST_DECRYPT_PROVIDER_BIN_ENV);
    register_protected_content_decrypt_provider(
        registry,
        &decrypt_binary,
        runtime_operation_issuer,
    )
    .await
    .unwrap();
}

#[cfg(unix)]
pub(crate) struct RuntimeCustodyProcessProviderFixture {
    _nodes_temp: tempfile::TempDir,
}

#[cfg(unix)]
pub(crate) fn runtime_custody_gateway_media_output_for_test() -> (Vec<u8>, Vec<Vec<u8>>) {
    clear_media_components(0x61)
}

#[cfg(unix)]
pub(crate) async fn register_runtime_custody_process_providers_for_test_registry(
    data_dir: &Path,
    registry: &Arc<ProviderRegistry>,
) -> RuntimeCustodyProcessProviderFixture {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let custody_binary = required_test_binary_path(TEST_CUSTODY_PROVIDER_BIN_ENV);
    let nodes_temp = tempfile::tempdir().unwrap();
    let nodes_root = fs::canonicalize(nodes_temp.path()).unwrap();
    owner_only_dir(data_dir);
    let (runtime_device_key, _) = elastos_identity::load_or_create_did(data_dir).unwrap();
    let runtime_issuer =
        RuntimeOperationIssuerKeyV1::new(runtime_device_key.verifying_key().to_bytes()).unwrap();
    let node1 = provisioned_process_custody_node_for_issuer(
        &custody_binary,
        &nodes_root,
        "node-1",
        runtime_issuer,
        digest(0xa1),
    );
    let node2 = provisioned_process_custody_node_for_issuer(
        &custody_binary,
        &nodes_root,
        "node-2",
        runtime_issuer,
        digest(0xa2),
    );
    let node3 = provisioned_process_custody_node_for_issuer(
        &custody_binary,
        &nodes_root,
        "node-3",
        runtime_issuer,
        digest(0xa3),
    );
    for fixture in [&node1, &node2, &node3] {
        fixture
            .registry
            .register_sub_provider(
                CHAIN_PROVIDER_ID,
                Arc::new(AllowingProcessChainEvidenceProvider),
            )
            .await
            .unwrap();
    }
    register_inactive_custody_provider(
        &node1.registry,
        &custody_binary,
        &nodes_root.join("node-1"),
    )
    .await
    .unwrap();
    register_inactive_custody_provider(
        &node2.registry,
        &custody_binary,
        &nodes_root.join("node-2"),
    )
    .await
    .unwrap();
    register_inactive_custody_provider(
        &node3.registry,
        &custody_binary,
        &nodes_root.join("node-3"),
    )
    .await
    .unwrap();

    let epoch = signed_custody_epoch_for_node_keys([
        (
            node1.provisioned.node_public_key,
            node1.provisioned.node_custody_public_key,
        ),
        (
            node2.provisioned.node_public_key,
            node2.provisioned.node_custody_public_key,
        ),
        (
            node3.provisioned.node_public_key,
            node3.provisioned.node_custody_public_key,
        ),
    ]);
    let fixtures_by_node = BTreeMap::from([
        (node1.provisioned.node_public_key, node1),
        (node2.provisioned.node_public_key, node2),
        (node3.provisioned.node_public_key, node3),
    ]);
    let ordered_fixtures = epoch
        .statement()
        .nodes()
        .iter()
        .map(|node| fixtures_by_node.get(&node.node_public_key()).unwrap())
        .collect::<Vec<_>>();
    let owner_state_roots = [
        ordered_fixtures[0].owner_state_root,
        ordered_fixtures[1].owner_state_root,
        ordered_fixtures[2].owner_state_root,
    ];
    let now = crate::auth::now_ts();
    let pool = signed_custody_pool_for_epoch(&epoch, (now.saturating_sub(60), now + 3_600));
    let authorization =
        signed_committee_authorization_for_epoch(pool.pool_identity().unwrap(), &epoch);
    write_owner_only_custody_composition_config(
        data_dir,
        &RuntimeCustodyCompositionConfigFile {
            schema: CUSTODY_COMPOSITION_SCHEMA_V1.to_string(),
            expected_policy_authority_base64: raw_b64_32(
                SigningKey::from_bytes(&[0x71; 32])
                    .verifying_key()
                    .to_bytes(),
            ),
            expected_committee_authorization_identity_base64: canonical_b64(
                &authorization.authorization_identity().unwrap(),
            ),
            signed_pool_base64: canonical_b64(&pool),
            signed_epoch_base64: canonical_b64(&epoch),
            signed_committee_authorization_base64: canonical_b64(&authorization),
            routes: process_custody_play_routes(&epoch, owner_state_roots),
        },
    );

    register_protect_provider(registry, &protect_binary)
        .await
        .unwrap();
    registry
        .register_runtime_provider_target(
            CUSTODY_PROVIDER_ID,
            Arc::new(LibraryProcessCustodyDispatcher {
                expected_issuer: runtime_issuer,
                nodes: ordered_fixtures
                    .iter()
                    .map(|fixture| {
                        (
                            fixture.provisioned.node_public_key,
                            fixture.registry.clone(),
                        )
                    })
                    .collect(),
            }),
        )
        .await
        .unwrap();
    registry
        .set_carrier_invoker(Arc::new(LoopbackCustodyCarrierInvoker {
            registry: Arc::downgrade(registry),
        }))
        .await;
    register_test_decrypt_provider(registry, runtime_issuer).await;
    RuntimeCustodyProcessProviderFixture {
        _nodes_temp: nodes_temp,
    }
}

#[cfg(unix)]
pub(crate) async fn register_runtime_custody_mock_media_provider_for_test_registry(
    data_dir: &Path,
    registry: &Arc<ProviderRegistry>,
) {
    let provider_root = data_dir.join("protected-content/media-provider");
    owner_only_dir(&provider_root);
    let staging_root = provider_root.join("staging");
    owner_only_dir(&staging_root);
    registry
        .register_runtime_provider_target(
            MEDIA_PROVIDER_ID,
            Arc::new(TestMediaPreparationProvider {
                staging_root,
                requests: Mutex::new(Vec::new()),
                response: TestMediaPreparationResponse::Prepared,
            }),
        )
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_fails_closed_without_launch_token() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-no-launch-token";
    write_device_key(&harness.data_dir, 0x21);
    let (_epoch, _composition_now) = write_library_publish_test_composition(&harness.data_dir);
    let (_proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let decrypt = PrepareOnlyCleanupDecryptProvider::new();
    harness
        .registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();
    let profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &profile_did,
        crate::auth::now_ts(),
    );
    let open = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: None,
            session_id: None,
            grant_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(
        open.to_string()
            .contains(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE),
        "{open}"
    );
    assert!(decrypt.requests().await.is_empty());
    assert!(super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id()
    )
    .unwrap()
    .is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_fails_closed_without_release_wallet() {
    let harness = runtime_custody_prebuy_availability_harness(
        0x61,
        ContentAvailabilityTestConfig::accepted_now(),
    )
    .await;
    let principal_id = "person:local:runtime-custody-open-no-release-wallet";
    write_device_key(&harness.data_dir, 0x21);
    let (_epoch, _composition_now) = write_library_publish_test_composition(&harness.data_dir);
    let (proof_binding_id, _) =
        install_profile_authority_keeping_device_key(&harness.data_dir, principal_id);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(harness.mint_id)
        .unwrap();
    let decrypt = PrepareOnlyCleanupDecryptProvider::new();
    harness
        .registry
        .register_runtime_provider_target(PROTECTED_CONTENT_DECRYPT_PROVIDER_ID, decrypt.clone())
        .await
        .unwrap();
    let profile_did = load_profile_did_for_test(&harness.data_dir, principal_id);
    persist_runtime_custody_purchase_for_mint(
        &harness.data_dir,
        &mint,
        principal_id,
        &profile_did,
        crate::auth::now_ts(),
    );
    let open = super::open_runtime_custody_viewer(
        &harness.data_dir,
        harness.registry.clone(),
        super::RuntimeCustodyViewerOpenInput {
            principal_id: principal_id.to_string(),
            mint_id: hex::encode(harness.mint_id.as_bytes()),
            launch_id: Some(TEST_VIEWER_LAUNCH_ID.to_string()),
            proof_binding_id: Some(proof_binding_id),
            session_id: Some("runtime-session:alpha".to_string()),
            grant_id: Some("grant:alpha".to_string()),
        },
    )
    .await
    .unwrap_err();
    assert!(
        open.to_string()
            .contains(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE),
        "{open}"
    );
    let record = super::load_runtime_custody_viewer_record(
        &harness.data_dir,
        principal_id,
        mint.draft().mint_id(),
    )
    .unwrap()
    .expect("expected durable viewer cleanup record");
    let requests = decrypt.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["op"], "prepare_recipient");
    assert_eq!(requests[1]["op"], "close_viewer_session");
    assert_eq!(requests[2]["op"], "cancel_prepared_recipient");
    assert_eq!(
        requests[1]["viewer_session_handle"].as_str(),
        requests[2]["prepared_recipient_handle"].as_str()
    );
    assert_eq!(
        record.lifecycle_status,
        super::RuntimeCustodyViewerLifecycleStatus::AlreadyAbsent
    );
}
