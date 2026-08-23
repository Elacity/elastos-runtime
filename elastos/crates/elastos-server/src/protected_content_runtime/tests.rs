use std::collections::BTreeMap;
use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
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
    RuntimeDecryptProvider, RuntimeMintCoordinator, RuntimeMintCoordinatorOutcome,
    RuntimeMintDraft, RuntimeMintJournal, RuntimeMintNodeBinding, RuntimeMintSelectedNode,
    RuntimeOpenError, RuntimeOpenViewerSessionInput, RuntimeProtectedContentPurchaseIntent,
    RuntimeProviderCallError, RuntimePurchaseEffectAuthority, RuntimeReleaseCoordinator,
    RuntimeReleaseCoordinatorOutcome, RuntimeReleaseJournal, RuntimeReleaseTerminalResult,
    RuntimeRightsProvider, RuntimeSelectedProvider, RuntimeVerifiedPurchaseEffect,
};
use elastos_runtime::provider::{
    bridge::ProviderConfig, CapsuleProvider, Provider, ProviderBridge, ProviderCarrierInvoker,
    ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ResourceRequest, ResourceResponse,
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
use tokio::sync::Mutex;
use x_wing::kem::{Decapsulator as _, KeyExport as _};
use x_wing::TryKeyInit as _;

use super::{
    invoke_json_provider, list_unresolved_runtime_releases, load_runtime_custody_composition,
    load_runtime_custody_composition_config, publish_runtime_custody_library_object,
    register_inactive_custody_provider, register_inactive_custody_sub_provider,
    register_protect_provider, resolve_runtime_rights_policy, runtime_mint_journal,
    runtime_protected_content_id, unresolved_release_audit_records, InactiveCustodyProvider,
    RuntimeCustodyComposition, RuntimeCustodyCompositionConfigFile,
    RuntimeCustodyLibraryPublishInput, RuntimeCustodyRegistryAdapter,
    RuntimeCustodyRouteBindingConfig, RuntimeCustodyRouteTransportConfig,
    RuntimeDecryptRegistryAdapter, CHAIN_PROTECTED_CONTENT_POLICY_SCHEMA_V1,
    CUSTODY_COMPOSITION_SCHEMA_V1, CUSTODY_PROVIDER_ID,
    RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE, RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE,
    RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE, RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE,
    RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE, RUNTIME_PROVIDER_ID,
};
use elastos_protected_content_contracts::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationIdentityV1,
    CustodyCommitteeAuthorizationStatementV1, CustodyEnvelopeManifestV1, CustodyEnvelopeV1,
    CustodyEpochIssuerKeyV1, CustodyEpochStatementV1, CustodyNodeProvisioningRecordV1,
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
    ProtectionSessionNodeV1, RightsProviderRequestV1, RightsProviderResponseV1,
    ValidatedCustodyProviderRequestV1, ValidatedRightsProviderRequestV1, ViewerMediaPartSelectorV1,
    MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
};

struct RecordingProvider {
    name: &'static str,
    requests: Mutex<Vec<Value>>,
    response: Value,
}

struct ProcessChainEvidenceProvider {
    expected_request: RightsProviderRequestV1,
    requests: Mutex<Vec<Value>>,
    has_access: bool,
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
    checked_at: u64,
    receipt_cid: Option<String>,
    receipt_object_identity: Option<String>,
    receipt_publisher_did: Option<String>,
    mutate_fetch_path: Option<String>,
    mutate_manifest_extra_file: bool,
}

impl ContentAvailabilityTestConfig {
    fn accepted() -> Self {
        Self {
            policy: "protected-content-replication/v1".to_string(),
            status: "network_available".to_string(),
            replicas: 3,
            checked_at: NOW,
            receipt_cid: None,
            receipt_object_identity: None,
            receipt_publisher_did: None,
            mutate_fetch_path: None,
            mutate_manifest_extra_file: false,
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
        Self::with_signing_key(SigningKey::from_bytes(&[seed; 32]), config)
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
            peer_selection: json!({}),
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

fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
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

fn media_components(seed: u8) -> (Vec<u8>, Vec<Vec<u8>>) {
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

fn policy_body() -> RightsPolicyBodyV1 {
    RightsPolicyBodyV1::new(
        "content:alpha",
        RightsActionV1::View,
        "view",
        elastos_protected_content_contracts::RightsSubjectSourceV1::WalletAddress,
        11155111,
        EvmContractAddressV1::new([0x11; 20]).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
        RightsObservationFinalityV1::new(12),
    )
    .unwrap()
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
    let pool = signed_custody_pool_for_epoch(epoch, (now.saturating_sub(10), now + 10));
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
        now + 45,
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
        now + 50,
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
        now + 40,
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
        112,
        has_access,
        evidence_issued_at,
        evidence_issued_at + MAX_RIGHTS_EVIDENCE_LIFETIME_SECS,
    )
    .unwrap();
    let bytes = evidence.canonical_bytes().unwrap();
    json!({
        "schema": "elastos.chain.protected-content-rights-evidence/v1",
        "chain_id": evidence.observed_chain_id(),
        "observed_block_number": evidence.observed_block_number(),
        "head_block_number": evidence.head_block_number(),
        "observed_block_hash": format!(
            "0x{}",
            hex::encode(evidence.observed_block_hash().as_bytes())
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
        &envelope,
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
    assert_eq!(recorded["_runtime_invocation"]["target"], "decrypt");
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
    let pool = signed_custody_pool_for_epoch(&epoch, (now.saturating_sub(10), now + 10));
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
                Digest32::new([0x50 + node_seed; 32]),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    RuntimeMintDraft::new(
        &init_segment,
        &encrypted_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
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
fn write_mock_custody_provider(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let binary = root.join("mock-custody-provider.sh");
    let request_log = root.join("mock-custody-provider.requests");
    let pid_file = root.join("mock-custody-provider.pid");
    let script = format!(
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$$\" >> '{}'\nwhile IFS= read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in\n    *'\"op\":\"shutdown\"'*) printf '%s\\n' '{{\"status\":\"ok\"}}'; exit 0 ;;\n    *'\"unexpected\"'* ) printf '%s\\n' '{{\"status\":\"error\",\"code\":\"invalid_request\"}}' ;;\n    *'\"op\":\"status\"'*) printf '%s\\n' '{{\"status\":\"ok\",\"data\":{{\"provider\":\"custody\",\"version\":\"test-version\",\"configured\":true,\"supported_operations\":[\"status\",\"provision_node_share\",\"release_contribution\",\"shutdown\"],\"request_schema\":\"req-schema\",\"response_schema\":\"resp-schema\"}}}}' ;;\n    *'\"op\":\"release_contribution\"'*) printf '%s\\n' '{{\"status\":\"ok\",\"data\":{{\"echo\":\"custody\"}}}}' ;;\n    *) printf '%s\\n' '{{\"status\":\"ok\",\"data\":{{\"echo\":\"init\"}}}}' ;;\n  esac\ndone\n",
        pid_file.display(),
        request_log.display()
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
    register_inactive_custody_sub_provider(registry.as_ref(), custody.clone())
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
    register_inactive_custody_sub_provider(registry.as_ref(), custody)
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
    register_inactive_custody_sub_provider(&registry, custody)
        .await
        .unwrap();
    let mut schemes = registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, vec!["custody".to_string(), "key".to_string()]);
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
    let mut schemes = registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, vec![CUSTODY_PROVIDER_ID.to_string()]);

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
    assert_eq!(requests.len(), 2);
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
    assert_eq!(requests.len(), 2);
    let init: Value = serde_json::from_str(&requests[0]).unwrap();
    assert_eq!(init["op"], "init");
    let forwarded: Value = serde_json::from_str(&requests[1]).unwrap();
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
    assert_eq!(status["version"], "test-version");
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
    assert_eq!(status["request_schema"], "req-schema");
    assert_eq!(status["response_schema"], "resp-schema");
}

#[cfg(unix)]
fn inactive_custody_runtime_envelope(op: &str, transport: &str) -> Value {
    json!({
        "schema": "elastos.provider.invocation/v1",
        "source": RUNTIME_PROVIDER_ID,
        "target": CUSTODY_PROVIDER_ID,
        "op": op,
        "capability": format!("provider:{RUNTIME_PROVIDER_ID}->{CUSTODY_PROVIDER_ID}:{op}"),
        "transport": transport,
        "carrier": Value::Null,
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
        .register_sub_provider("custody", custody.clone())
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
        .register_sub_provider("custody", custody.clone())
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
    .unwrap();
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
            .unregister_sub_provider(CUSTODY_PROVIDER_ID)
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
        .register_sub_provider("protect", protect_provider)
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

    let placeholder_operation = make_signed_runtime_release_operation_for_envelope_and_epoch_at(
        0x21,
        &envelope,
        custody_epoch.clone(),
        crate::auth::now_ts(),
    );
    let (placeholder_wallet_request, placeholder_wallet_response) =
        wallet_request_response_for_release_at(
            &placeholder_operation,
            "profile:alpha",
            "wallet-account-alpha",
            "wallet-request:11111111111111111111111111111111",
            crate::auth::now_ts(),
        );
    let purchase_effect = runtime_verified_purchase_effect_for_mint(
        &mint,
        "profile:alpha",
        "wallet-account-alpha",
        "wallet-request:11111111111111111111111111111111",
        0xaa,
        crate::auth::now_ts(),
    );
    let preliminary_buy = bind_buy(
        &mint,
        &placeholder_wallet_request,
        &placeholder_wallet_response,
        &purchase_effect,
        crate::auth::now_ts(),
    )
    .unwrap();
    assert_eq!(preliminary_buy.binding(), &binding_for_envelope(&envelope));
    assert_eq!(preliminary_buy.action(), RightsActionV1::View);

    let decrypt_registry = Arc::new(ProviderRegistry::new());
    let decrypt_bridge = ProviderBridge::spawn(
        &decrypt_binary,
        ProviderConfig {
            extra: json!({
                "trusted_runtime_issuer": runtime_issuer_hex(0x21),
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let decrypt_provider: Arc<dyn Provider> = Arc::new(CapsuleProvider::with_scheme(
        Arc::new(decrypt_bridge),
        "decrypt",
    ));
    decrypt_registry
        .register_sub_provider("decrypt", decrypt_provider)
        .await
        .unwrap();
    let decrypt = RuntimeDecryptRegistryAdapter::new(decrypt_registry.clone());

    let audit_a = RuntimeReleaseAuditIdV1::new(digest(0xa1)).unwrap();
    let audit_b = RuntimeReleaseAuditIdV1::new(digest(0xa2)).unwrap();
    let flow_now = crate::auth::now_ts();
    let prepare_issued_at = flow_now.saturating_sub(5);
    let prepare_expires_at = flow_now + 60;
    let prepared_a = prepare_recipient(
        &decrypt,
        &preliminary_buy,
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
        audit_b,
        runtime_operation_issuer_for_seed(0x21),
        prepare_issued_at,
        prepare_expires_at,
    )
    .await
    .unwrap();

    let operation = make_signed_runtime_release_operation_for_envelope_and_epoch_and_recipient_at(
        0x21,
        &envelope,
        custody_epoch.clone(),
        prepared_a.recipient_public_key().clone(),
        prepared_a.recipient_identity().clone(),
        audit_a,
        flow_now,
    );
    let (wallet_request, wallet_response) = wallet_request_response_for_release_at(
        &operation,
        "profile:alpha",
        "wallet-account-alpha",
        "wallet-request:11111111111111111111111111111111",
        flow_now,
    );
    let buy = bind_buy(
        &mint,
        &wallet_request,
        &wallet_response,
        &purchase_effect,
        flow_now,
    )
    .unwrap();

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
    .unwrap();
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
    .unwrap();
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
            custody_envelope: &envelope,
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
            custody_envelope: &envelope,
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
            custody_envelope: &envelope,
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
        .unregister_sub_provider("decrypt")
        .await
        .unwrap();
    let mut schemes = decrypt_registry.sub_provider_schemes().await;
    schemes.sort();
    assert_eq!(schemes, Vec::<String>::new());
    protect_registry
        .unregister_sub_provider("protect")
        .await
        .unwrap();
    let mut protect_schemes = protect_registry.sub_provider_schemes().await;
    protect_schemes.sort();
    assert_eq!(protect_schemes, Vec::<String>::new());
    assert_eq!(
        prepare_recipient(
            &decrypt,
            &buy,
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
            .unregister_sub_provider(CUSTODY_PROVIDER_ID)
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
        .unregister_sub_provider(CUSTODY_PROVIDER_ID)
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
        .unregister_sub_provider(CUSTODY_PROVIDER_ID)
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
        .unregister_sub_provider(CUSTODY_PROVIDER_ID)
        .await
        .unwrap();
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
        .register_sub_provider("decrypt", decrypt.clone())
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
        .register_sub_provider("decrypt", decrypt.clone())
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
        .register_sub_provider("decrypt", read_provider.clone())
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
    registry.unregister_sub_provider("decrypt").await.unwrap();

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
        .register_sub_provider("decrypt", cancel_provider.clone())
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
    registry.unregister_sub_provider("decrypt").await.unwrap();

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
        .register_sub_provider("decrypt", close_provider.clone())
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
            .register_sub_provider("decrypt", decrypt)
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
    let encrypted_content = EncryptedContentIdentityV1::new(digest(0x51), 4096).unwrap();
    let content_id = runtime_protected_content_id(&encrypted_content).unwrap();
    let policy = RightsPolicyBodyV1::new(
        content_id.clone(),
        RightsActionV1::View,
        "view",
        elastos_protected_content_contracts::RightsSubjectSourceV1::WalletAddress,
        11155111,
        EvmContractAddressV1::new([0x11; 20]).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
        RightsObservationFinalityV1::new(12),
    )
    .unwrap();
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
    let resolved =
        resolve_runtime_rights_policy(registry.as_ref(), &encrypted_content, RightsActionV1::View)
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
        RightsPolicyBodyV1::new(
            "content:deadbeef".to_string(),
            RightsActionV1::View,
            "view",
            elastos_protected_content_contracts::RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(12),
        )
        .unwrap(),
        RightsPolicyBodyV1::new(
            content_id.clone(),
            RightsActionV1::Download,
            "download",
            elastos_protected_content_contracts::RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(12),
        )
        .unwrap(),
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
            &encrypted_content,
            RightsActionV1::View
        )
        .await
        .is_err());
    }
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
        clear_init_segment,
        clear_segments,
        source_storage: "plain_localhost_root".to_string(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_fails_closed_without_protect_provider() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    write_owner_only_custody_composition_config(
        &data_dir,
        &custody_composition_config(now, library_publish_test_routes(&epoch)),
    );
    let error = publish_runtime_custody_library_object(
        &data_dir,
        Arc::new(ProviderRegistry::new()),
        library_publish_test_input("person:local:runtime-custody-missing-protect"),
    )
    .await
    .expect_err("missing protect provider must fail closed");
    assert!(
        error
            .to_string()
            .contains("Runtime custody protect provider is unavailable"),
        "{error}"
    );
    assert!(!data_dir.join("protected-content/runtime-mint").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_fails_closed_without_chain_policy() {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    write_owner_only_custody_composition_config(
        &data_dir,
        &custody_composition_config(now, library_publish_test_routes(&epoch)),
    );
    let registry = Arc::new(ProviderRegistry::new());
    register_protect_provider(&registry, &protect_binary)
        .await
        .unwrap();
    let error = publish_runtime_custody_library_object(
        &data_dir,
        registry,
        library_publish_test_input("person:local:runtime-custody-missing-policy"),
    )
    .await
    .expect_err("missing chain policy must fail closed");
    assert!(
        error
            .to_string()
            .contains("Runtime custody rights policy is unavailable"),
        "{error}"
    );
    assert!(!data_dir.join("protected-content/runtime-mint").exists());
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
        let content_id = request
            .get("content_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Provider("missing content_id".to_string()))?;
        if request.get("action").and_then(Value::as_str) != Some("view") {
            return Err(ProviderError::Provider(
                "unexpected policy action".to_string(),
            ));
        }
        let policy = RightsPolicyBodyV1::new(
            content_id,
            RightsActionV1::View,
            "view",
            elastos_protected_content_contracts::RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(12),
        )
        .map_err(|_| ProviderError::Provider("library mint policy is invalid".to_string()))?;
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
        _route: &ProviderCarrierRoute,
        invocation: &ProviderInvocation,
        request: Value,
    ) -> Result<Value, ProviderError> {
        let registry = self.registry.upgrade().ok_or_else(|| {
            ProviderError::Provider("library mint carrier loopback registry is gone".to_string())
        })?;
        registry.send_raw(&invocation.target, &request).await
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
        registry.send_raw(CUSTODY_PROVIDER_ID, request).await
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
                ))
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
fn write_device_key(data_dir: &Path, seed: u8) {
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
async fn write_library_object_bytes(
    registry: &ProviderRegistry,
    principal_id: &str,
    uri: &str,
    bytes: &[u8],
) {
    let response = registry
        .send_raw(
            "object",
            &json!({
                "op": "write",
                "principal_id": principal_id,
                "uri": uri,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await
        .unwrap();
    assert_eq!(response["status"], "ok", "{response}");
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_publish_protects_mints_and_records_identity_only_facts() {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let pool = signed_custody_pool_for_epoch(&epoch, (now.saturating_sub(60), now + 3_600));
    let authorization =
        signed_committee_authorization_for_epoch(pool.pool_identity().unwrap(), &epoch);
    write_owner_only_custody_composition_config(
        &data_dir,
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
            routes: custody_route_bindings(
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
        },
    );

    let registry = Arc::new(ProviderRegistry::new());
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
    registry
        .register_sub_provider(
            "object",
            Arc::new(crate::library::ObjectProvider::new(
                data_dir.clone(),
                Arc::downgrade(&registry),
            )),
        )
        .await
        .unwrap();

    let principal_id = "person:local:runtime-custody-slice-c";
    crate::auth::store_test_principal_root_protection(&data_dir, principal_id);
    let root = crate::auth::principal_localhost_root(principal_id);
    let uri = format!("{root}/Documents/protected-clear-media");
    let (clear_init, clear_segments) = clear_media_components(0x41);
    write_library_object_bytes(
        &registry,
        principal_id,
        &format!("{uri}/init.mp4"),
        &clear_init,
    )
    .await;
    for (index, segment) in clear_segments.iter().enumerate() {
        write_library_object_bytes(
            &registry,
            principal_id,
            &format!("{uri}/segments/{index:08}.m4s"),
            segment,
        )
        .await;
    }

    let publish = registry
        .send_raw(
            "object",
            &json!({
                "op": "publish",
                "principal_id": principal_id,
                "uri": uri,
                "protection": {
                    "mode": "runtime_custody",
                    "mime_type": MEDIA_MIME_TYPE_V1,
                    "codecs": MEDIA_CODECS_V1,
                },
            }),
        )
        .await
        .unwrap();
    assert_eq!(publish["status"], "ok", "{publish}");
    let cid = publish["data"]["cid"].as_str().unwrap().to_string();
    let content_id = publish["data"]["content_security"]["content_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mint_id = publish["data"]["content_security"]["mint_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        publish["data"]["content_security"]["published_payload"],
        "runtime_custody_encrypted"
    );
    assert_eq!(publish["data"]["object"]["published_cid"], cid);
    assert_eq!(
        publish["data"]["object"]["metadata"]["protected_content"]["content_id"],
        content_id
    );
    assert!(!publish.to_string().contains("sealed"));
    assert!(!publish.to_string().contains("cek"));

    let status = registry
        .send_raw(
            "object",
            &json!({
                "op": "status",
                "principal_id": principal_id,
                "uri": uri,
            }),
        )
        .await
        .unwrap();
    assert_eq!(status["status"], "ok", "{status}");
    assert_eq!(status["data"]["object"]["published_cid"], cid);
    assert_eq!(
        status["data"]["published"]["content_security"]["content_id"],
        content_id
    );
    assert_eq!(
        status["data"]["published"]["content_security"]["mint_id"],
        mint_id
    );
    assert!(status["data"]["published"]["content_security"]
        .get("sealed_object")
        .is_none());

    let share = registry
        .send_raw(
            "object",
            &json!({
                "op": "share",
                "principal_id": principal_id,
                "uri": uri,
                "recipients": ["did:key:zShare"],
                "policy": "public",
            }),
        )
        .await
        .unwrap();
    assert_eq!(share["status"], "error");
    assert!(
        share["message"]
            .as_str()
            .unwrap()
            .contains("Runtime custody sharing is not available yet"),
        "{share}"
    );

    let mint_root = data_dir.join("protected-content/runtime-mint");
    assert!(mint_root.is_dir());
    assert!(!any_file_contains(&mint_root, &clear_init));
    assert!(!any_file_contains(&mint_root, &clear_segments[0]));
    let records_root = elastos_common::localhost::rooted_localhost_fs_path(
        &data_dir,
        &format!("{root}/.AppData/LocalHost/.Runtime/Library/Published")
            .strip_prefix("localhost://")
            .unwrap(),
    )
    .unwrap();
    if records_root.exists() {
        assert!(!any_file_contains(&records_root, &clear_init));
        assert!(!any_file_contains(&records_root, &clear_segments[0]));
    }
}

#[cfg(unix)]
struct LibraryRuntimeCustodyHarness {
    _temp: tempfile::TempDir,
    _nodes_temp: Option<tempfile::TempDir>,
    data_dir: PathBuf,
    registry: Arc<ProviderRegistry>,
    creator: String,
    buyer: String,
    mint_id: String,
    cid: String,
    content_id: String,
    clear_init: Vec<u8>,
    clear_segments: Vec<Vec<u8>>,
}

#[cfg(unix)]
async fn publish_library_runtime_custody_harness() -> LibraryRuntimeCustodyHarness {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let now = crate::auth::now_ts();
    let epoch = signed_custody_epoch();
    let pool = signed_custody_pool_for_epoch(&epoch, (now.saturating_sub(60), now + 3_600));
    let authorization =
        signed_committee_authorization_for_epoch(pool.pool_identity().unwrap(), &epoch);
    write_owner_only_custody_composition_config(
        &data_dir,
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
            routes: library_publish_test_routes(&epoch),
        },
    );

    let registry = Arc::new(ProviderRegistry::new());
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
    registry
        .register_sub_provider(
            "object",
            Arc::new(crate::library::ObjectProvider::new(
                data_dir.clone(),
                Arc::downgrade(&registry),
            )),
        )
        .await
        .unwrap();

    let creator = "person:local:runtime-custody-slice-d-creator";
    let buyer = "person:local:runtime-custody-slice-d-buyer";
    crate::auth::store_test_principal_root_protection(&data_dir, creator);
    crate::auth::store_test_principal_root_protection(&data_dir, buyer);
    let root = crate::auth::principal_localhost_root(creator);
    let uri = format!("{root}/Documents/protected-clear-media");
    let (clear_init, clear_segments) = clear_media_components(0x41);
    write_library_object_bytes(&registry, creator, &format!("{uri}/init.mp4"), &clear_init).await;
    for (index, segment) in clear_segments.iter().enumerate() {
        write_library_object_bytes(
            &registry,
            creator,
            &format!("{uri}/segments/{index:08}.m4s"),
            segment,
        )
        .await;
    }
    let publish = registry
        .send_raw(
            "object",
            &json!({
                "op": "publish",
                "principal_id": creator,
                "uri": uri,
                "protection": {
                    "mode": "runtime_custody",
                    "mime_type": MEDIA_MIME_TYPE_V1,
                    "codecs": MEDIA_CODECS_V1,
                },
            }),
        )
        .await
        .unwrap();
    assert_eq!(publish["status"], "ok", "{publish}");
    LibraryRuntimeCustodyHarness {
        _temp: temp,
        _nodes_temp: None,
        data_dir,
        registry,
        creator: creator.to_string(),
        buyer: buyer.to_string(),
        mint_id: publish["data"]["content_security"]["mint_id"]
            .as_str()
            .unwrap()
            .to_string(),
        cid: publish["data"]["cid"].as_str().unwrap().to_string(),
        content_id: publish["data"]["content_security"]["content_id"]
            .as_str()
            .unwrap()
            .to_string(),
        clear_init,
        clear_segments,
    }
}

#[cfg(unix)]
fn wallet_buy_bundle_for_mint(
    mint: &PersistedRuntimeMint,
    principal_id: &str,
    now: u64,
) -> (String, String, Value) {
    wallet_buy_bundle_for_mint_with_profile(
        mint,
        principal_id,
        now,
        ProfileIdentityV1::from_public_key_bytes(
            SigningKey::from_bytes(&[0x26; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
    )
}

#[cfg(unix)]
fn wallet_buy_bundle_for_mint_with_profile(
    mint: &PersistedRuntimeMint,
    principal_id: &str,
    now: u64,
    profile: ProfileIdentityV1,
) -> (String, String, Value) {
    let binding = ProtectedContentBindingV1::new(
        mint.draft().encrypted_content().clone(),
        mint.draft().key_envelope().clone(),
        mint.draft().policy().clone(),
        profile,
        wallet(7),
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap();
    let request = RightsRequestV1::new(
        binding,
        RightsActionV1::View,
        recipient_identity(0x30),
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
    let signed = WalletSignedRightsRequestV1::new(request.clone(), signature_bytes).unwrap();
    let context = VerifiedWalletInvocationContext::new(
        principal_id,
        "runtime-session:alpha",
        Some("proof:alpha".to_string()),
        "grant:alpha",
        "runtime",
        "launch:alpha",
    )
    .unwrap();
    let account_id = "wallet-account-alpha";
    let approval_request_id = "wallet-request:11111111111111111111111111111111";
    let wallet_request = WalletProviderRequestV2::new(
        &context,
        approval_request_id,
        now,
        now + 120,
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            account_id: account_id.to_string(),
            canonical_rights_request_hex: hex::encode(request.canonical_bytes().unwrap()),
            reason: "Buy protected content".to_string(),
        },
    )
    .unwrap();
    let result = ProtectedContentRightsSignatureResultV1::new(
        account_id,
        wallet_address_hex(wallet(7)),
        hex::encode(signed.canonical_bytes().unwrap()),
    )
    .unwrap();
    let wallet_response = WalletProviderResponseV2::for_request(
        &wallet_request,
        WalletResultV2::Ok {
            data: serde_json::to_value(result).unwrap(),
        },
    );
    (
        hex::encode(serde_json::to_vec(&wallet_request).unwrap()),
        hex::encode(serde_json::to_vec(&wallet_response).unwrap()),
        json!({
            "account_id": account_id,
            "address": wallet_address_hex(wallet(7)),
            "approval_request_id": approval_request_id,
            "chain_namespace": "eip155:20",
            "network": "esc-mainnet",
            "to": "0x2222222222222222222222222222222222222222",
            "value": "0x1",
            "data": "0x",
            "wallet_binding": {
                "kind": "managed_signed",
                "signed_transaction_sha256": format!("sha256:{}", hex::encode([0xab; 32])),
            },
            "transaction_hash": format!("0x{}", hex::encode([0xaa; 32])),
            "chain_observation": {
                "schema": "elastos.chain.broadcast_receipt/v1",
                "network": "esc-mainnet",
            },
            "confirmed_at": now,
        }),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_buy_is_denied_before_purchase() {
    let harness = publish_library_runtime_custody_harness().await;
    let denied = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
            }),
        )
        .await
        .unwrap();
    assert_eq!(denied["status"], "error", "{denied}");
    assert!(
        denied["message"]
            .as_str()
            .unwrap_or_default()
            .contains(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE),
        "{denied}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_buy_binds_wallet_chain_and_marks_listing_buyer_owned() {
    let harness = publish_library_runtime_custody_harness().await;
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(Digest32::new(
            hex::decode(&harness.mint_id).unwrap().try_into().unwrap(),
        ))
        .unwrap();
    let now = crate::auth::now_ts();
    let (wallet_request_hex, wallet_response_hex, purchase) =
        wallet_buy_bundle_for_mint(&mint, &harness.buyer, now);
    let buy = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "wallet_request_hex": wallet_request_hex,
                "wallet_response_hex": wallet_response_hex,
                "purchase": purchase,
            }),
        )
        .await
        .unwrap();
    assert_eq!(buy["status"], "ok", "{buy}");
    assert_eq!(buy["data"]["mint_id"], harness.mint_id);
    assert_eq!(buy["data"]["content_id"], harness.content_id);
    assert_eq!(buy["data"]["cid"], harness.cid);
    assert_eq!(buy["data"]["availability"]["status"], "buyer_owned");
    assert!(!buy.to_string().contains("sealed"));
    assert!(!buy.to_string().contains("cek"));
    assert!(!buy.to_string().contains("play_url"));

    let listings = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "list_runtime_custody",
                "principal_id": harness.buyer,
            }),
        )
        .await
        .unwrap();
    assert_eq!(listings["status"], "ok", "{listings}");
    let listing = listings["data"]["listings"]
        .as_array()
        .and_then(|items| items.first())
        .expect("published listing");
    assert_eq!(listing["mint_id"], harness.mint_id);
    assert_eq!(listing["publisher_principal_id"], harness.creator);
    assert_eq!(listing["buyer_principal_id"], harness.buyer);
    assert_eq!(listing["availability"]["status"], "buyer_owned");
    assert!(!listings.to_string().contains("sealed"));
    assert!(!listings
        .to_string()
        .contains(&hex::encode(&harness.clear_init)));

    let stranger_listings = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "list_runtime_custody",
                "principal_id": "person:local:runtime-custody-slice-d-stranger",
            }),
        )
        .await
        .unwrap();
    assert_eq!(stranger_listings["status"], "ok", "{stranger_listings}");
    assert!(
        stranger_listings["data"]["listings"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(false),
        "{stranger_listings}"
    );

    let second_buy = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": "person:local:runtime-custody-slice-d-stranger",
                "mint_id": harness.mint_id,
                "wallet_request_hex": wallet_request_hex,
                "wallet_response_hex": wallet_response_hex,
                "purchase": purchase,
            }),
        )
        .await
        .unwrap();
    assert_eq!(second_buy["status"], "error", "{second_buy}");
    assert!(
        second_buy["message"]
            .as_str()
            .unwrap_or_default()
            .contains(RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE),
        "{second_buy}"
    );

    let purchases_root = harness.data_dir.join("protected-content/runtime-purchases");
    if purchases_root.exists() {
        assert!(!any_file_contains(&purchases_root, &harness.clear_init));
        assert!(!any_file_contains(
            &purchases_root,
            &harness.clear_segments[0]
        ));
    }
    let mint_root = harness.data_dir.join("protected-content/runtime-mint");
    assert!(!any_file_contains(&mint_root, &harness.clear_init));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_is_denied_before_purchase() {
    let harness = publish_library_runtime_custody_harness().await;
    let denied = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "open_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
            }),
        )
        .await
        .unwrap();
    assert_eq!(denied["status"], "error", "{denied}");
    assert!(
        denied["message"]
            .as_str()
            .unwrap_or_default()
            .contains(RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE),
        "{denied}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_fails_closed_without_decrypt() {
    let harness = publish_library_runtime_custody_harness().await;
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(Digest32::new(
            hex::decode(&harness.mint_id).unwrap().try_into().unwrap(),
        ))
        .unwrap();
    let now = crate::auth::now_ts();
    let (wallet_request_hex, wallet_response_hex, purchase) =
        wallet_buy_bundle_for_mint(&mint, &harness.buyer, now);
    let buy = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "wallet_request_hex": wallet_request_hex,
                "wallet_response_hex": wallet_response_hex,
                "purchase": purchase,
            }),
        )
        .await
        .unwrap();
    assert_eq!(buy["status"], "ok", "{buy}");
    let open = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "open_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
            }),
        )
        .await
        .unwrap();
    assert_eq!(open["status"], "error", "{open}");
    assert!(
        open["message"]
            .as_str()
            .unwrap_or_default()
            .contains(RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE),
        "{open}"
    );
}

#[cfg(unix)]
fn derived_device_runtime_issuer_hex(seed: u8) -> String {
    format!(
        "0x{}",
        hex::encode(derived_device_runtime_issuer(seed).as_bytes())
    )
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
async fn register_test_decrypt_provider(registry: &Arc<ProviderRegistry>, issuer_hex: &str) {
    let decrypt_binary = required_test_binary_path(TEST_DECRYPT_PROVIDER_BIN_ENV);
    let decrypt_bridge = ProviderBridge::spawn(
        &decrypt_binary,
        ProviderConfig {
            extra: json!({
                "trusted_runtime_issuer": issuer_hex,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let decrypt_provider: Arc<dyn Provider> = Arc::new(CapsuleProvider::with_scheme(
        Arc::new(decrypt_bridge),
        "decrypt",
    ));
    registry
        .register_sub_provider("decrypt", decrypt_provider)
        .await
        .unwrap();
}

#[cfg(unix)]
async fn publish_library_runtime_custody_play_harness() -> LibraryRuntimeCustodyHarness {
    let protect_binary = required_test_binary_path(TEST_PROTECT_PROVIDER_BIN_ENV);
    let custody_binary = required_test_binary_path(TEST_CUSTODY_PROVIDER_BIN_ENV);
    let temp = tempfile::tempdir().unwrap();
    let nodes_temp = tempfile::tempdir().unwrap();
    let nodes_root = fs::canonicalize(nodes_temp.path()).unwrap();
    let data_dir = temp.path().join("data");
    owner_only_dir(&data_dir);
    write_device_key(&data_dir, 0x21);
    let runtime_issuer = derived_device_runtime_issuer(0x21);
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
        &data_dir,
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

    let registry = Arc::new(ProviderRegistry::new());
    register_protect_provider(&registry, &protect_binary)
        .await
        .unwrap();
    registry
        .register_sub_provider(
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
            registry: Arc::downgrade(&registry),
        }))
        .await;
    registry
        .register_sub_provider(CHAIN_PROVIDER_ID, Arc::new(LibraryMintChainPolicyProvider))
        .await
        .unwrap();
    register_test_decrypt_provider(&registry, &derived_device_runtime_issuer_hex(0x21)).await;
    registry
        .register_sub_provider("wallet", Arc::new(LibraryReleaseWalletProvider))
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
    registry
        .register_sub_provider(
            "object",
            Arc::new(crate::library::ObjectProvider::new(
                data_dir.clone(),
                Arc::downgrade(&registry),
            )),
        )
        .await
        .unwrap();

    let creator = "person:local:runtime-custody-slice-d-creator";
    let buyer = "person:local:runtime-custody-slice-d-buyer";
    crate::auth::store_test_principal_root_protection(&data_dir, creator);
    crate::auth::store_test_principal_root_protection(&data_dir, buyer);
    let root = crate::auth::principal_localhost_root(creator);
    let uri = format!("{root}/Documents/protected-clear-media");
    let (clear_init, clear_segments) = clear_media_components(0x41);
    write_library_object_bytes(&registry, creator, &format!("{uri}/init.mp4"), &clear_init).await;
    for (index, segment) in clear_segments.iter().enumerate() {
        write_library_object_bytes(
            &registry,
            creator,
            &format!("{uri}/segments/{index:08}.m4s"),
            segment,
        )
        .await;
    }
    let publish = registry
        .send_raw(
            "object",
            &json!({
                "op": "publish",
                "principal_id": creator,
                "uri": uri,
                "protection": {
                    "mode": "runtime_custody",
                    "mime_type": MEDIA_MIME_TYPE_V1,
                    "codecs": MEDIA_CODECS_V1,
                },
            }),
        )
        .await
        .unwrap();
    assert_eq!(publish["status"], "ok", "{publish}");
    LibraryRuntimeCustodyHarness {
        _temp: temp,
        _nodes_temp: Some(nodes_temp),
        data_dir,
        registry,
        creator: creator.to_string(),
        buyer: buyer.to_string(),
        mint_id: publish["data"]["content_security"]["mint_id"]
            .as_str()
            .unwrap()
            .to_string(),
        cid: publish["data"]["cid"].as_str().unwrap().to_string(),
        content_id: publish["data"]["content_security"]["content_id"]
            .as_str()
            .unwrap()
            .to_string(),
        clear_init,
        clear_segments,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_fails_closed_without_launch_token() {
    let harness = publish_library_runtime_custody_harness().await;
    register_test_decrypt_provider(&harness.registry, &derived_device_runtime_issuer_hex(0x21))
        .await;
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(Digest32::new(
            hex::decode(&harness.mint_id).unwrap().try_into().unwrap(),
        ))
        .unwrap();
    let now = crate::auth::now_ts();
    let (wallet_request_hex, wallet_response_hex, purchase) =
        wallet_buy_bundle_for_mint(&mint, &harness.buyer, now);
    let buy = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "wallet_request_hex": wallet_request_hex,
                "wallet_response_hex": wallet_response_hex,
                "purchase": purchase,
            }),
        )
        .await
        .unwrap();
    assert_eq!(buy["status"], "ok", "{buy}");
    let open = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "open_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
            }),
        )
        .await
        .unwrap();
    assert_eq!(open["status"], "error", "{open}");
    assert!(
        open["message"]
            .as_str()
            .unwrap_or_default()
            .contains(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE),
        "{open}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_fails_closed_without_release_wallet() {
    let harness = publish_library_runtime_custody_harness().await;
    register_test_decrypt_provider(&harness.registry, &derived_device_runtime_issuer_hex(0x21))
        .await;
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(Digest32::new(
            hex::decode(&harness.mint_id).unwrap().try_into().unwrap(),
        ))
        .unwrap();
    let now = crate::auth::now_ts();
    let (wallet_request_hex, wallet_response_hex, purchase) =
        wallet_buy_bundle_for_mint(&mint, &harness.buyer, now);
    let buy = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "wallet_request_hex": wallet_request_hex,
                "wallet_response_hex": wallet_response_hex,
                "purchase": purchase,
            }),
        )
        .await
        .unwrap();
    assert_eq!(buy["status"], "ok", "{buy}");
    let open = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "open_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "proof_binding_id": "proof:passkey:missing-wallet",
                "session_id": "runtime-session:alpha",
                "grant_id": "grant:alpha",
            }),
        )
        .await
        .unwrap();
    assert_eq!(open["status"], "error", "{open}");
    assert!(
        open["message"]
            .as_str()
            .unwrap_or_default()
            .contains(RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE),
        "{open}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_custody_library_open_after_buy_reads_clear_media_and_closes() {
    let harness = publish_library_runtime_custody_play_harness().await;
    let (proof_binding_id, profile) =
        install_profile_authority_keeping_device_key(&harness.data_dir, &harness.buyer);
    let mint = runtime_mint_journal(&harness.data_dir)
        .load(Digest32::new(
            hex::decode(&harness.mint_id).unwrap().try_into().unwrap(),
        ))
        .unwrap();
    let now = crate::auth::now_ts();
    let (wallet_request_hex, wallet_response_hex, purchase) =
        wallet_buy_bundle_for_mint_with_profile(&mint, &harness.buyer, now, profile);
    let buy = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "buy",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "wallet_request_hex": wallet_request_hex,
                "wallet_response_hex": wallet_response_hex,
                "purchase": purchase,
            }),
        )
        .await
        .unwrap();
    assert_eq!(buy["status"], "ok", "{buy}");
    let open = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "open_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "proof_binding_id": proof_binding_id,
                "session_id": "runtime-session:alpha",
                "grant_id": "grant:alpha",
            }),
        )
        .await
        .unwrap();
    assert_eq!(open["status"], "ok", "{open}");
    assert_eq!(
        open["data"]["schema"],
        "elastos.library.runtime-custody-viewer/v1"
    );
    let handle = open["data"]["viewer_session_handle"]
        .as_str()
        .unwrap()
        .to_string();
    let init = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "read_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "viewer_session_handle": handle,
            }),
        )
        .await
        .unwrap();
    assert_eq!(init["status"], "ok", "{init}");
    assert_eq!(init["data"]["encoding"], "base64");
    let init_bytes = base64::engine::general_purpose::STANDARD
        .decode(init["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(init_bytes, harness.clear_init);
    let segment = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "read_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "viewer_session_handle": handle,
                "segment_index": 0,
            }),
        )
        .await
        .unwrap();
    assert_eq!(segment["status"], "ok", "{segment}");
    let segment_bytes = base64::engine::general_purpose::STANDARD
        .decode(segment["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(segment_bytes, harness.clear_segments[0]);
    let closed = harness
        .registry
        .send_raw(
            "object",
            &json!({
                "op": "close_viewer",
                "principal_id": harness.buyer,
                "mint_id": harness.mint_id,
                "viewer_session_handle": handle,
            }),
        )
        .await
        .unwrap();
    assert_eq!(closed["status"], "ok", "{closed}");
    assert!(!open.to_string().contains("cek"));
    assert!(!open.to_string().contains("play_url"));
    assert!(!init.to_string().contains(&hex::encode(&harness.clear_init)));
    let mint_root = harness.data_dir.join("protected-content/runtime-mint");
    assert!(!any_file_contains(&mint_root, &harness.clear_init));
    assert!(!any_file_contains(&mint_root, &harness.clear_segments[0]));
    let purchases_root = harness.data_dir.join("protected-content/runtime-purchases");
    if purchases_root.exists() {
        assert!(!any_file_contains(&purchases_root, &harness.clear_init));
        assert!(!any_file_contains(
            &purchases_root,
            &harness.clear_segments[0]
        ));
    }
    let listings_root = harness.data_dir.join("protected-content/runtime-listings");
    if listings_root.exists() {
        assert!(!any_file_contains(&listings_root, &harness.clear_init));
    }
}
