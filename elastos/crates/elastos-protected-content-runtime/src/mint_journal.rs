//! Runtime-owned producer/mint journal for one media flow.
//!
//! Persists bound identities and per-node provision receipts only. It does not
//! store CEKs, share bytes, routes, or topology. Partial provisioning is a
//! durable terminal abort. Custody provisioning is not content availability or
//! listing. First-release orphan policy is
//! bounded retention: accepted shares stay unreachable until a separately
//! reviewed retirement operation exists.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use elastos_protected_content_contracts::{
    CanonicalContract, ContentAccessIdV1, CustodyCommitteeAuthorizationIdentityV1,
    CustodyEpochIdentityV1, CustodyNodeProvisioningRecordIdentityV1, CustodyPoolFailureDomainIdV1,
    CustodyPoolIdentityV1, CustodyPoolOperatorIdV1, Digest32, EncryptedContentIdentityV1,
    KeyEnvelopeIdentityV1, NodePublicKey, NodeSetV1, RightsPolicyIdentityV1,
    RuntimeCustodyProvisioningIdV1, ThresholdV1,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
};
use nix::fcntl::{Flock, FlockArg};
use nix::unistd::geteuid;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

const STORE_MAGIC: &[u8; 8] = b"epc-mj04";
const STORE_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/runtime-mint-journal/v4";
const INTENT_MAGIC: &[u8; 8] = b"epc-mi01";
const INTENT_DIGEST_DOMAIN: &[u8] = b"elastos/protected-content/runtime-mint-intent/v1";
const STORE_LOCK_FILE: &str = "runtime-mint-journal.lock";
const MINT_ID_DOMAIN: &[u8] = b"elastos.protected-content.runtime-mint-id/v4";
const MINT_INTENT_REQUEST_ID_DOMAIN: &[u8] =
    b"elastos.protected-content.runtime-mint-intent-request-id/v1";
const MINT_INTENT_SOURCE_BINDING_DOMAIN: &[u8] =
    b"elastos.protected-content.runtime-mint-intent-source-binding/v1";
const MAX_STORE_FILE_BYTES: usize = 64 * 1024;
const REQUIRED_NODES: usize = 3;
const MAX_AVAILABILITY_TEXT_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeMintJournalError {
    #[error("runtime mint journal is unavailable")]
    Unavailable,
    #[error("runtime mint journal record is corrupt")]
    Corrupt,
    #[error("runtime mint journal record conflicts with existing authority")]
    Conflict,
    #[error("runtime mint selection is invalid")]
    InvalidSelection,
    #[error("runtime mint operation was not found")]
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCustodyTerminalKind {
    CustodyProvisioned,
    AbortedPartialProvision,
}

/// Runtime-selected requirements for one existing content-provider availability
/// receipt. This is configuration/selection, not provider evidence.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeContentAvailabilityRequirement {
    expected_provider_did: String,
    expected_object_identity: String,
    expected_publisher_did: String,
    policy: String,
    minimum_replicas: u32,
    max_age_seconds: u64,
    max_future_skew_seconds: u64,
}

impl fmt::Debug for RuntimeContentAvailabilityRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeContentAvailabilityRequirement")
            .field("expected_provider_did", &self.expected_provider_did)
            .field("expected_object_identity", &self.expected_object_identity)
            .field("expected_publisher_did", &self.expected_publisher_did)
            .field("policy", &self.policy)
            .field("minimum_replicas", &self.minimum_replicas)
            .field("max_age_seconds", &self.max_age_seconds)
            .field("max_future_skew_seconds", &self.max_future_skew_seconds)
            .finish()
    }
}

impl RuntimeContentAvailabilityRequirement {
    pub fn new(
        expected_provider_did: impl Into<String>,
        expected_object_identity: impl Into<String>,
        expected_publisher_did: impl Into<String>,
        policy: impl Into<String>,
        minimum_replicas: u32,
        max_age_seconds: u64,
        max_future_skew_seconds: u64,
    ) -> Result<Self, RuntimeMintJournalError> {
        let value = Self {
            expected_provider_did: expected_provider_did.into(),
            expected_object_identity: expected_object_identity.into(),
            expected_publisher_did: expected_publisher_did.into(),
            policy: policy.into(),
            minimum_replicas,
            max_age_seconds,
            max_future_skew_seconds,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), RuntimeMintJournalError> {
        validate_availability_text(&self.expected_provider_did)?;
        validate_availability_text(&self.expected_object_identity)?;
        validate_availability_text(&self.expected_publisher_did)?;
        validate_availability_text(&self.policy)?;
        if self.minimum_replicas == 0
            || self.max_age_seconds == 0
            || self.max_future_skew_seconds > self.max_age_seconds
        {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        Ok(())
    }

    pub fn expected_provider_did(&self) -> &str {
        &self.expected_provider_did
    }

    pub fn expected_object_identity(&self) -> &str {
        &self.expected_object_identity
    }

    pub fn expected_publisher_did(&self) -> &str {
        &self.expected_publisher_did
    }

    pub fn policy(&self) -> &str {
        &self.policy
    }

    pub const fn minimum_replicas(&self) -> u32 {
        self.minimum_replicas
    }

    pub const fn max_age_seconds(&self) -> u64 {
        self.max_age_seconds
    }

    pub const fn max_future_skew_seconds(&self) -> u64 {
        self.max_future_skew_seconds
    }
}

/// Identity-only result of server-side verification of an existing signed
/// `elastos://content` availability receipt. Its fields are intentionally
/// private and it cannot deserialize provider JSON.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeVerifiedContentAvailability {
    content_cid: String,
    object_identity: String,
    publisher_identity: String,
    expected_provider_did: String,
    policy: String,
    required_replicas: u32,
    observed_replicas: u32,
    checked_at: u64,
    receipt_digest: Digest32,
    encrypted_content: EncryptedContentIdentityV1,
    media_manifest_root: Digest32,
}

impl fmt::Debug for RuntimeVerifiedContentAvailability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeVerifiedContentAvailability")
            .field("content_cid", &self.content_cid)
            .field("object_identity", &self.object_identity)
            .field("publisher_identity", &self.publisher_identity)
            .field("expected_provider_did", &self.expected_provider_did)
            .field("policy", &self.policy)
            .field("required_replicas", &self.required_replicas)
            .field("observed_replicas", &self.observed_replicas)
            .field("checked_at", &self.checked_at)
            .field("receipt_digest", &self.receipt_digest)
            .field("encrypted_content", &self.encrypted_content)
            .field("media_manifest_root", &self.media_manifest_root)
            .finish()
    }
}

impl RuntimeVerifiedContentAvailability {
    #[allow(
        clippy::too_many_arguments,
        reason = "these are the exact independently verified content receipt bindings"
    )]
    pub fn new(
        content_cid: impl Into<String>,
        object_identity: impl Into<String>,
        publisher_identity: impl Into<String>,
        requirement: &RuntimeContentAvailabilityRequirement,
        observed_replicas: u32,
        checked_at: u64,
        receipt_digest: Digest32,
        encrypted_content: EncryptedContentIdentityV1,
        media_manifest_root: Digest32,
    ) -> Result<Self, RuntimeMintJournalError> {
        requirement.validate()?;
        let value = Self {
            content_cid: content_cid.into(),
            object_identity: object_identity.into(),
            publisher_identity: publisher_identity.into(),
            expected_provider_did: requirement.expected_provider_did.clone(),
            policy: requirement.policy.clone(),
            required_replicas: requirement.minimum_replicas,
            observed_replicas,
            checked_at,
            receipt_digest,
            encrypted_content,
            media_manifest_root,
        };
        value.validate()?;
        if value.object_identity != requirement.expected_object_identity
            || value.publisher_identity != requirement.expected_publisher_did
        {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        Ok(value)
    }

    fn validate(&self) -> Result<(), RuntimeMintJournalError> {
        validate_availability_text(&self.content_cid)?;
        validate_availability_text(&self.object_identity)?;
        validate_availability_text(&self.publisher_identity)?;
        validate_availability_text(&self.expected_provider_did)?;
        validate_availability_text(&self.policy)?;
        if self.required_replicas == 0
            || self.observed_replicas < self.required_replicas
            || self.checked_at == 0
            || self.receipt_digest == Digest32::new([0; 32])
            || self.media_manifest_root == Digest32::new([0; 32])
        {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        self.encrypted_content
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::InvalidSelection)?;
        Ok(())
    }

    fn matches_requirement(&self, requirement: &RuntimeContentAvailabilityRequirement) -> bool {
        self.expected_provider_did == requirement.expected_provider_did
            && self.object_identity == requirement.expected_object_identity
            && self.publisher_identity == requirement.expected_publisher_did
            && self.policy == requirement.policy
            && self.required_replicas == requirement.minimum_replicas
    }

    fn matches_draft(&self, draft: &RuntimeMintDraft) -> bool {
        self.encrypted_content == *draft.encrypted_content()
            && self.media_manifest_root == draft.media_identity().media_manifest_root()
    }

    pub fn content_cid(&self) -> &str {
        &self.content_cid
    }

    pub fn object_identity(&self) -> &str {
        &self.object_identity
    }

    pub fn publisher_identity(&self) -> &str {
        &self.publisher_identity
    }

    pub fn expected_provider_did(&self) -> &str {
        &self.expected_provider_did
    }

    pub fn policy(&self) -> &str {
        &self.policy
    }

    pub const fn required_replicas(&self) -> u32 {
        self.required_replicas
    }

    pub const fn observed_replicas(&self) -> u32 {
        self.observed_replicas
    }

    pub const fn checked_at(&self) -> u64 {
        self.checked_at
    }

    pub const fn receipt_digest(&self) -> Digest32 {
        self.receipt_digest
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub const fn media_manifest_root(&self) -> Digest32 {
        self.media_manifest_root
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeMintNodeBinding {
    node_public_key: NodePublicKey,
    operator_id: CustodyPoolOperatorIdV1,
    failure_domain_id: CustodyPoolFailureDomainIdV1,
    owner_state_root: Digest32,
}

impl fmt::Debug for RuntimeMintNodeBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintNodeBinding")
            .field("node_public_key", &self.node_public_key)
            .field("operator_id", &self.operator_id)
            .field("failure_domain_id", &self.failure_domain_id)
            .field("owner_state_root", &self.owner_state_root)
            .finish()
    }
}

impl RuntimeMintNodeBinding {
    pub fn new(
        node_public_key: NodePublicKey,
        operator_id: CustodyPoolOperatorIdV1,
        failure_domain_id: CustodyPoolFailureDomainIdV1,
        owner_state_root: Digest32,
    ) -> Result<Self, RuntimeMintJournalError> {
        if owner_state_root == Digest32::new([0; 32]) {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        Ok(Self {
            node_public_key,
            operator_id,
            failure_domain_id,
            owner_state_root,
        })
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn operator_id(&self) -> CustodyPoolOperatorIdV1 {
        self.operator_id
    }

    pub const fn failure_domain_id(&self) -> CustodyPoolFailureDomainIdV1 {
        self.failure_domain_id
    }

    pub const fn owner_state_root(&self) -> Digest32 {
        self.owner_state_root
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeMintIntent {
    request_id: Digest32,
    principal_id: String,
    source_binding_digest: Digest32,
    mime_type: String,
    codecs: String,
    clear_init_sha256: Digest32,
    clear_segment_sha256: Vec<Digest32>,
    content_access_id: ContentAccessIdV1,
    protect_state: RuntimeMintIntentProtectState,
    custody_pool: CustodyPoolIdentityV1,
    custody_epoch: CustodyEpochIdentityV1,
    custody_committee_authorization: CustodyCommitteeAuthorizationIdentityV1,
    nodes: Vec<RuntimeMintNodeBinding>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeMintIntentProtectState {
    NotStarted,
    OpenRequestPending,
    OpenHandlePendingCancel([u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]),
    OpenHandlePendingClose([u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]),
    SettledBeforeDraft(RuntimeMintIntentProtectSettlement),
    Completed(Digest32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeMintIntentProtectSettlement {
    Cancelled,
    Closed,
    AlreadyAbsent,
}

impl fmt::Debug for RuntimeMintIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintIntent")
            .field("request_id", &self.request_id)
            .field("principal_id", &"[redacted]")
            .field("source_binding_digest", &self.source_binding_digest)
            .field("mime_type", &self.mime_type)
            .field("codecs", &self.codecs)
            .field("clear_init_sha256", &self.clear_init_sha256)
            .field("clear_segment_count", &self.clear_segment_sha256.len())
            .field("content_access_id", &"[redacted]")
            .field("protect_state", &self.protect_state_label())
            .field(
                "protect_terminal_settlement",
                &self.protect_terminal_settlement_label(),
            )
            .field("protect_completed", &self.completed_mint_id().is_some())
            .field("custody_pool", &self.custody_pool)
            .field("custody_epoch", &self.custody_epoch)
            .field(
                "custody_committee_authorization",
                &self.custody_committee_authorization,
            )
            .field("node_count", &self.nodes.len())
            .finish()
    }
}

impl RuntimeMintIntent {
    pub fn request_id_for_source(
        principal_id: &str,
        object_uri: &str,
        source_storage: &str,
    ) -> Result<Digest32, RuntimeMintJournalError> {
        validate_intent_text(principal_id)?;
        Ok(compute_mint_intent_request_id(
            principal_id,
            compute_source_binding_digest(object_uri, source_storage),
        ))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the pre-effect intent must bind source digests, declaration, access id, and committee identities exactly once"
    )]
    pub fn new(
        principal_id: impl Into<String>,
        object_uri: &str,
        source_storage: &str,
        mime_type: impl Into<String>,
        codecs: impl Into<String>,
        clear_init_segment: &[u8],
        clear_segments: &[Vec<u8>],
        content_access_id: ContentAccessIdV1,
        custody_pool: CustodyPoolIdentityV1,
        custody_epoch: CustodyEpochIdentityV1,
        custody_committee_authorization: CustodyCommitteeAuthorizationIdentityV1,
        nodes: Vec<RuntimeMintNodeBinding>,
    ) -> Result<Self, RuntimeMintJournalError> {
        let principal_id = principal_id.into();
        let mime_type = mime_type.into();
        let codecs = codecs.into();
        validate_intent_text(&principal_id)?;
        validate_intent_text(&mime_type)?;
        validate_intent_text(&codecs)?;
        let source_binding_digest = compute_source_binding_digest(object_uri, source_storage);
        let request_id = compute_mint_intent_request_id(&principal_id, source_binding_digest);
        let clear_init_sha256 = Digest32::new(Sha256::digest(clear_init_segment).into());
        let clear_segment_sha256 = clear_segments
            .iter()
            .map(|segment| Digest32::new(Sha256::digest(segment).into()))
            .collect::<Vec<_>>();
        let value = Self {
            request_id,
            principal_id,
            source_binding_digest,
            mime_type,
            codecs,
            clear_init_sha256,
            clear_segment_sha256,
            content_access_id,
            protect_state: RuntimeMintIntentProtectState::NotStarted,
            custody_pool,
            custody_epoch,
            custody_committee_authorization,
            nodes,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), RuntimeMintJournalError> {
        validate_intent_text(&self.principal_id)?;
        validate_intent_text(&self.mime_type)?;
        validate_intent_text(&self.codecs)?;
        if self.request_id == Digest32::new([0; 32])
            || self.source_binding_digest == Digest32::new([0; 32])
            || self.clear_init_sha256 == Digest32::new([0; 32])
            || self.clear_segment_sha256.is_empty()
            || self.nodes.len() != REQUIRED_NODES
        {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        match self.protect_state {
            RuntimeMintIntentProtectState::OpenHandlePendingCancel(handle)
            | RuntimeMintIntentProtectState::OpenHandlePendingClose(handle) => {
                if handle == [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
                    return Err(RuntimeMintJournalError::InvalidSelection);
                }
            }
            RuntimeMintIntentProtectState::Completed(mint_id)
                if mint_id == Digest32::new([0; 32]) =>
            {
                return Err(RuntimeMintJournalError::InvalidSelection);
            }
            _ => {}
        }
        self.custody_pool
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::InvalidSelection)?;
        self.custody_epoch
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::InvalidSelection)?;
        self.custody_committee_authorization
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::InvalidSelection)?;
        let mut node_keys = std::collections::BTreeSet::new();
        let mut operators = std::collections::BTreeSet::new();
        let mut domains = std::collections::BTreeSet::new();
        let mut roots = std::collections::BTreeSet::new();
        for digest in &self.clear_segment_sha256 {
            if *digest == Digest32::new([0; 32]) {
                return Err(RuntimeMintJournalError::InvalidSelection);
            }
        }
        for node in &self.nodes {
            if !node_keys.insert(node.node_public_key)
                || !operators.insert(node.operator_id)
                || !domains.insert(node.failure_domain_id)
                || !roots.insert(node.owner_state_root)
            {
                return Err(RuntimeMintJournalError::InvalidSelection);
            }
        }
        Ok(())
    }

    pub const fn request_id(&self) -> Digest32 {
        self.request_id
    }

    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub const fn source_binding_digest(&self) -> Digest32 {
        self.source_binding_digest
    }

    pub fn same_authority_as(&self, other: &Self) -> bool {
        self.request_id == other.request_id
            && self.principal_id == other.principal_id
            && self.source_binding_digest == other.source_binding_digest
            && self.mime_type == other.mime_type
            && self.codecs == other.codecs
            && self.clear_init_sha256 == other.clear_init_sha256
            && self.clear_segment_sha256 == other.clear_segment_sha256
            && self.content_access_id == other.content_access_id
            && self.custody_pool == other.custody_pool
            && self.custody_epoch == other.custody_epoch
            && self.custody_committee_authorization == other.custody_committee_authorization
            && self.nodes == other.nodes
    }

    pub const fn provider_effect_started(&self) -> bool {
        !matches!(
            self.protect_state,
            RuntimeMintIntentProtectState::NotStarted
        )
    }

    pub const fn protect_open_request_pending(&self) -> bool {
        matches!(
            self.protect_state,
            RuntimeMintIntentProtectState::OpenRequestPending
        )
    }

    pub fn protect_pending_cancel_handle(
        &self,
    ) -> Option<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]> {
        match self.protect_state {
            RuntimeMintIntentProtectState::OpenHandlePendingCancel(handle) => Some(handle),
            _ => None,
        }
    }

    pub fn protect_pending_close_handle(
        &self,
    ) -> Option<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]> {
        match self.protect_state {
            RuntimeMintIntentProtectState::OpenHandlePendingClose(handle) => Some(handle),
            _ => None,
        }
    }

    pub const fn protect_terminal_before_draft(&self) -> bool {
        matches!(
            self.protect_state,
            RuntimeMintIntentProtectState::SettledBeforeDraft(_)
        )
    }

    pub const fn completed_mint_id(&self) -> Option<Digest32> {
        match self.protect_state {
            RuntimeMintIntentProtectState::Completed(mint_id) => Some(mint_id),
            _ => None,
        }
    }

    pub const fn protect_state_label(&self) -> &'static str {
        match self.protect_state {
            RuntimeMintIntentProtectState::NotStarted => "not_started",
            RuntimeMintIntentProtectState::OpenRequestPending => "open_request_pending",
            RuntimeMintIntentProtectState::OpenHandlePendingCancel(_) => {
                "open_handle_pending_cancel"
            }
            RuntimeMintIntentProtectState::OpenHandlePendingClose(_) => "open_handle_pending_close",
            RuntimeMintIntentProtectState::SettledBeforeDraft(_) => "settled_before_draft",
            RuntimeMintIntentProtectState::Completed(_) => "completed",
        }
    }

    pub const fn protect_terminal_settlement_label(&self) -> Option<&'static str> {
        match self.protect_state {
            RuntimeMintIntentProtectState::SettledBeforeDraft(
                RuntimeMintIntentProtectSettlement::Cancelled,
            ) => Some("cancelled"),
            RuntimeMintIntentProtectState::SettledBeforeDraft(
                RuntimeMintIntentProtectSettlement::Closed,
            ) => Some("closed"),
            RuntimeMintIntentProtectState::SettledBeforeDraft(
                RuntimeMintIntentProtectSettlement::AlreadyAbsent,
            ) => Some("already_absent"),
            _ => None,
        }
    }

    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    pub fn codecs(&self) -> &str {
        &self.codecs
    }

    pub const fn clear_init_sha256(&self) -> Digest32 {
        self.clear_init_sha256
    }

    pub fn clear_segment_sha256(&self) -> &[Digest32] {
        &self.clear_segment_sha256
    }

    pub const fn content_access_id(&self) -> ContentAccessIdV1 {
        self.content_access_id
    }

    pub const fn custody_pool(&self) -> CustodyPoolIdentityV1 {
        self.custody_pool
    }

    pub const fn custody_epoch(&self) -> CustodyEpochIdentityV1 {
        self.custody_epoch
    }

    pub const fn custody_committee_authorization(&self) -> CustodyCommitteeAuthorizationIdentityV1 {
        self.custody_committee_authorization
    }

    pub fn nodes(&self) -> &[RuntimeMintNodeBinding] {
        &self.nodes
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeMintNodeReceipt {
    node_public_key: NodePublicKey,
    provisioning_id: RuntimeCustodyProvisioningIdV1,
    record_identity: CustodyNodeProvisioningRecordIdentityV1,
    owner_state_root: Digest32,
}

impl fmt::Debug for RuntimeMintNodeReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintNodeReceipt")
            .field("node_public_key", &self.node_public_key)
            .field("provisioning_id", &self.provisioning_id)
            .field("record_identity", &self.record_identity)
            .field("owner_state_root", &self.owner_state_root)
            .finish()
    }
}

impl RuntimeMintNodeReceipt {
    pub fn new(
        node_public_key: NodePublicKey,
        provisioning_id: RuntimeCustodyProvisioningIdV1,
        record_identity: CustodyNodeProvisioningRecordIdentityV1,
        owner_state_root: Digest32,
    ) -> Result<Self, RuntimeMintJournalError> {
        if owner_state_root == Digest32::new([0; 32]) {
            return Err(RuntimeMintJournalError::Corrupt);
        }
        Ok(Self {
            node_public_key,
            provisioning_id,
            record_identity,
            owner_state_root,
        })
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn provisioning_id(&self) -> RuntimeCustodyProvisioningIdV1 {
        self.provisioning_id
    }

    pub fn record_identity(&self) -> CustodyNodeProvisioningRecordIdentityV1 {
        self.record_identity
    }

    pub const fn owner_state_root(&self) -> Digest32 {
        self.owner_state_root
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeMintDraft {
    mint_id: Digest32,
    media_identity: CencFmp4MediaIdentityV1,
    content_access_id: ContentAccessIdV1,
    key_envelope: KeyEnvelopeIdentityV1,
    policy: RightsPolicyIdentityV1,
    content_key_commitment: Digest32,
    threshold: ThresholdV1,
    nodes: Vec<RuntimeMintNodeBinding>,
}

impl fmt::Debug for RuntimeMintDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintDraft")
            .field("mint_id", &self.mint_id)
            .field("media_identity", &self.media_identity)
            .field("content_access_id", &self.content_access_id)
            .field("key_envelope", &self.key_envelope)
            .field("policy", &self.policy)
            .field("content_key_commitment", &self.content_key_commitment)
            .field("threshold", &self.threshold)
            .field("node_count", &self.nodes.len())
            .finish()
    }
}

impl RuntimeMintDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        init_segment: &[u8],
        encrypted_segments: &[Vec<u8>],
        mime_type: impl Into<String>,
        codecs: impl Into<String>,
        content_access_id: ContentAccessIdV1,
        key_envelope: KeyEnvelopeIdentityV1,
        policy: RightsPolicyIdentityV1,
        content_key_commitment: Digest32,
        threshold: ThresholdV1,
        nodes: Vec<RuntimeMintNodeBinding>,
    ) -> Result<Self, RuntimeMintJournalError> {
        let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
            init_segment,
            encrypted_segments,
            mime_type,
            codecs,
        )
        .map_err(|_| RuntimeMintJournalError::InvalidSelection)?;
        Self::new_recorded(
            media_identity,
            content_access_id,
            key_envelope,
            policy,
            content_key_commitment,
            threshold,
            nodes,
        )
    }

    fn new_recorded(
        media_identity: CencFmp4MediaIdentityV1,
        content_access_id: ContentAccessIdV1,
        key_envelope: KeyEnvelopeIdentityV1,
        policy: RightsPolicyIdentityV1,
        content_key_commitment: Digest32,
        threshold: ThresholdV1,
        nodes: Vec<RuntimeMintNodeBinding>,
    ) -> Result<Self, RuntimeMintJournalError> {
        if threshold.required() != 2 || threshold.total() != 3 {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        if nodes.len() != REQUIRED_NODES {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        if content_key_commitment == Digest32::new([0; 32]) {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        if key_envelope.encrypted_content() != media_identity.encrypted_content() {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        if key_envelope.threshold() != threshold {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        let node_set = NodeSetV1::new(
            threshold,
            nodes.iter().map(|node| node.node_public_key).collect(),
        )
        .map_err(|_| RuntimeMintJournalError::InvalidSelection)?;
        if node_set
            .node_set_id()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?
            != key_envelope.node_set_id()
        {
            return Err(RuntimeMintJournalError::InvalidSelection);
        }
        let mut node_keys = std::collections::BTreeSet::new();
        let mut operators = std::collections::BTreeSet::new();
        let mut domains = std::collections::BTreeSet::new();
        let mut roots = std::collections::BTreeSet::new();
        for node in &nodes {
            if !node_keys.insert(node.node_public_key)
                || !operators.insert(node.operator_id)
                || !domains.insert(node.failure_domain_id)
                || !roots.insert(node.owner_state_root)
            {
                return Err(RuntimeMintJournalError::InvalidSelection);
            }
        }
        let mut draft = Self {
            mint_id: Digest32::new([0; 32]),
            media_identity,
            content_access_id,
            key_envelope,
            policy,
            content_key_commitment,
            threshold,
            nodes,
        };
        draft.mint_id = draft.compute_mint_id()?;
        Ok(draft)
    }

    pub const fn mint_id(&self) -> Digest32 {
        self.mint_id
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        self.media_identity.encrypted_content()
    }

    pub const fn content_access_id(&self) -> ContentAccessIdV1 {
        self.content_access_id
    }

    pub const fn media_identity(&self) -> &CencFmp4MediaIdentityV1 {
        &self.media_identity
    }

    pub fn key_envelope(&self) -> &KeyEnvelopeIdentityV1 {
        &self.key_envelope
    }

    pub fn policy(&self) -> &RightsPolicyIdentityV1 {
        &self.policy
    }

    pub const fn content_key_commitment(&self) -> Digest32 {
        self.content_key_commitment
    }

    pub const fn threshold(&self) -> ThresholdV1 {
        self.threshold
    }

    pub fn nodes(&self) -> &[RuntimeMintNodeBinding] {
        &self.nodes
    }

    pub fn pool(&self) -> CustodyPoolIdentityV1 {
        self.key_envelope.custody_pool()
    }

    pub fn epoch(&self) -> CustodyEpochIdentityV1 {
        self.key_envelope.custody_epoch()
    }

    pub fn committee(&self) -> CustodyCommitteeAuthorizationIdentityV1 {
        self.key_envelope.custody_committee_authorization()
    }

    fn compute_mint_id(&self) -> Result<Digest32, RuntimeMintJournalError> {
        let mut hasher = Sha256::new();
        hasher.update(MINT_ID_DOMAIN);
        hasher.update(
            &self
                .media_identity
                .canonical_bytes()
                .map_err(|_| RuntimeMintJournalError::Corrupt)?,
        );
        hasher.update(self.content_access_id.as_bytes());
        hasher.update(
            &self
                .key_envelope
                .canonical_bytes()
                .map_err(|_| RuntimeMintJournalError::Corrupt)?,
        );
        hasher.update(
            &self
                .policy
                .canonical_bytes()
                .map_err(|_| RuntimeMintJournalError::Corrupt)?,
        );
        hasher.update(self.content_key_commitment.as_bytes());
        hasher.update([self.threshold.required(), self.threshold.total()]);
        for node in &self.nodes {
            hasher.update(node.node_public_key.as_bytes());
            hasher.update(node.operator_id.as_bytes());
            hasher.update(node.failure_domain_id.as_bytes());
            hasher.update(node.owner_state_root.as_bytes());
        }
        Ok(Digest32::new(hasher.finalize().into()))
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MintNodeState {
    binding: RuntimeMintNodeBinding,
    effect_started: bool,
    receipt: Option<RuntimeMintNodeReceipt>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PersistedRuntimeMint {
    draft: RuntimeMintDraft,
    node_states: Vec<MintNodeState>,
    custody_terminal: Option<RuntimeCustodyTerminalKind>,
    content_availability: Option<RuntimeVerifiedContentAvailability>,
}

impl fmt::Debug for PersistedRuntimeMint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistedRuntimeMint")
            .field("mint_id", &self.draft.mint_id)
            .field("custody_terminal", &self.custody_terminal)
            .field("content_availability", &self.content_availability)
            .finish()
    }
}

impl PersistedRuntimeMint {
    pub fn draft(&self) -> &RuntimeMintDraft {
        &self.draft
    }

    pub const fn custody_terminal(&self) -> Option<RuntimeCustodyTerminalKind> {
        self.custody_terminal
    }

    pub fn content_availability(&self) -> Option<&RuntimeVerifiedContentAvailability> {
        self.content_availability.as_ref()
    }

    pub fn accepted_orphans(&self) -> Vec<&RuntimeMintNodeReceipt> {
        self.node_states
            .iter()
            .filter_map(|state| state.receipt.as_ref())
            .collect()
    }

    pub fn node_effect_started(&self, node_public_key: NodePublicKey) -> bool {
        self.node_states
            .iter()
            .any(|state| state.binding.node_public_key == node_public_key && state.effect_started)
    }

    pub fn any_effect_started(&self) -> bool {
        self.node_states.iter().any(|state| state.effect_started)
    }

    pub fn all_receipts_present(&self) -> bool {
        self.node_states.iter().all(|state| state.receipt.is_some())
    }
}

pub struct RuntimeMintJournal {
    root_dir: PathBuf,
    lock_path: PathBuf,
}

impl fmt::Debug for RuntimeMintJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeMintJournal")
            .field("root_dir", &"[redacted]")
            .finish()
    }
}

impl RuntimeMintJournal {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        Self {
            lock_path: root_dir.join(STORE_LOCK_FILE),
            root_dir,
        }
    }

    pub fn persist_bound(
        &self,
        draft: &RuntimeMintDraft,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let record = PersistedRuntimeMint {
            draft: draft.clone(),
            node_states: draft
                .nodes
                .iter()
                .map(|binding| MintNodeState {
                    binding: binding.clone(),
                    effect_started: false,
                    receipt: None,
                })
                .collect(),
            custody_terminal: None,
            content_availability: None,
        };
        self.write_or_replay(&record)
    }

    pub fn persist_intent(
        &self,
        intent: &RuntimeMintIntent,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        self.write_or_replay_intent(intent)
    }

    pub fn load(&self, mint_id: Digest32) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        self.read_record(mint_id)
    }

    pub fn load_intent(
        &self,
        request_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        self.read_intent(request_id)
    }

    pub fn mark_intent_protect_effect_started(
        &self,
        request_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut intent = self.read_intent(request_id)?;
        match intent.protect_state {
            RuntimeMintIntentProtectState::NotStarted => {
                intent.protect_state = RuntimeMintIntentProtectState::OpenRequestPending;
                self.write_replace_intent(&intent)?;
                Ok(intent)
            }
            RuntimeMintIntentProtectState::OpenRequestPending => Ok(intent),
            _ => Err(RuntimeMintJournalError::Conflict),
        }
    }

    pub fn mark_intent_protect_opened(
        &self,
        request_id: Digest32,
        handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut intent = self.read_intent(request_id)?;
        match intent.protect_state {
            RuntimeMintIntentProtectState::OpenRequestPending => {
                intent.protect_state =
                    RuntimeMintIntentProtectState::OpenHandlePendingCancel(handle);
                self.write_replace_intent(&intent)?;
                Ok(intent)
            }
            RuntimeMintIntentProtectState::OpenHandlePendingCancel(existing)
                if existing == handle =>
            {
                Ok(intent)
            }
            _ => Err(RuntimeMintJournalError::Conflict),
        }
    }

    pub fn mark_intent_protect_finalized(
        &self,
        request_id: Digest32,
        handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut intent = self.read_intent(request_id)?;
        match intent.protect_state {
            RuntimeMintIntentProtectState::OpenHandlePendingCancel(existing)
                if existing == handle =>
            {
                intent.protect_state =
                    RuntimeMintIntentProtectState::OpenHandlePendingClose(handle);
                self.write_replace_intent(&intent)?;
                Ok(intent)
            }
            RuntimeMintIntentProtectState::OpenHandlePendingClose(existing)
                if existing == handle =>
            {
                Ok(intent)
            }
            _ => Err(RuntimeMintJournalError::Conflict),
        }
    }

    pub fn mark_intent_protect_cancelled_before_draft(
        &self,
        request_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        self.mark_intent_protect_terminal_before_draft(
            request_id,
            RuntimeMintIntentProtectSettlement::Cancelled,
        )
    }

    pub fn mark_intent_protect_closed_before_draft(
        &self,
        request_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        self.mark_intent_protect_terminal_before_draft(
            request_id,
            RuntimeMintIntentProtectSettlement::Closed,
        )
    }

    pub fn mark_intent_protect_already_absent_before_draft(
        &self,
        request_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        self.mark_intent_protect_terminal_before_draft(
            request_id,
            RuntimeMintIntentProtectSettlement::AlreadyAbsent,
        )
    }

    pub fn mark_intent_completed(
        &self,
        request_id: Digest32,
        mint_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut intent = self.read_intent(request_id)?;
        match intent.protect_state {
            RuntimeMintIntentProtectState::SettledBeforeDraft(_) => {
                intent.protect_state = RuntimeMintIntentProtectState::Completed(mint_id);
                self.write_replace_intent(&intent)?;
                Ok(intent)
            }
            RuntimeMintIntentProtectState::Completed(existing) if existing == mint_id => Ok(intent),
            _ => Err(RuntimeMintJournalError::Conflict),
        }
    }

    fn mark_intent_protect_terminal_before_draft(
        &self,
        request_id: Digest32,
        settlement: RuntimeMintIntentProtectSettlement,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut intent = self.read_intent(request_id)?;
        match intent.protect_state {
            RuntimeMintIntentProtectState::OpenRequestPending
            | RuntimeMintIntentProtectState::OpenHandlePendingCancel(_)
            | RuntimeMintIntentProtectState::OpenHandlePendingClose(_) => {
                intent.protect_state =
                    RuntimeMintIntentProtectState::SettledBeforeDraft(settlement);
                self.write_replace_intent(&intent)?;
                Ok(intent)
            }
            RuntimeMintIntentProtectState::SettledBeforeDraft(existing)
                if existing == settlement =>
            {
                Ok(intent)
            }
            _ => Err(RuntimeMintJournalError::Conflict),
        }
    }

    pub fn mark_node_effect_started(
        &self,
        mint_id: Digest32,
        node_public_key: NodePublicKey,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut record = self.read_record(mint_id)?;
        if record.custody_terminal.is_some() {
            return Err(RuntimeMintJournalError::Conflict);
        }
        let node = record
            .node_states
            .iter_mut()
            .find(|state| state.binding.node_public_key == node_public_key)
            .ok_or(RuntimeMintJournalError::InvalidSelection)?;
        node.effect_started = true;
        self.write_replace(&record)?;
        Ok(record)
    }

    pub fn mark_node_receipt(
        &self,
        mint_id: Digest32,
        receipt: RuntimeMintNodeReceipt,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut record = self.read_record(mint_id)?;
        if record.custody_terminal.is_some() {
            return Err(RuntimeMintJournalError::Conflict);
        }
        let node = record
            .node_states
            .iter_mut()
            .find(|state| state.binding.node_public_key == receipt.node_public_key)
            .ok_or(RuntimeMintJournalError::InvalidSelection)?;
        if !node.effect_started {
            return Err(RuntimeMintJournalError::Conflict);
        }
        if node.binding.owner_state_root != receipt.owner_state_root {
            return Err(RuntimeMintJournalError::Conflict);
        }
        if let Some(existing) = &node.receipt {
            if existing != &receipt {
                return Err(RuntimeMintJournalError::Conflict);
            }
            return Ok(record);
        }
        node.receipt = Some(receipt);
        self.write_replace(&record)?;
        Ok(record)
    }

    pub fn mark_custody_provisioned(
        &self,
        mint_id: Digest32,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut record = self.read_record(mint_id)?;
        if !record.all_receipts_present() {
            return Err(RuntimeMintJournalError::Conflict);
        }
        match record.custody_terminal {
            Some(RuntimeCustodyTerminalKind::CustodyProvisioned) => return Ok(record),
            Some(_) => return Err(RuntimeMintJournalError::Conflict),
            None => {}
        }
        record.custody_terminal = Some(RuntimeCustodyTerminalKind::CustodyProvisioned);
        self.write_replace(&record)?;
        Ok(record)
    }

    pub fn mark_aborted_partial_provision(
        &self,
        mint_id: Digest32,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut record = self.read_record(mint_id)?;
        match record.custody_terminal {
            Some(RuntimeCustodyTerminalKind::AbortedPartialProvision) => return Ok(record),
            Some(_) => return Err(RuntimeMintJournalError::Conflict),
            None => {}
        }
        record.custody_terminal = Some(RuntimeCustodyTerminalKind::AbortedPartialProvision);
        self.write_replace(&record)?;
        Ok(record)
    }

    pub fn mark_content_available(
        &self,
        mint_id: Digest32,
        requirement: &RuntimeContentAvailabilityRequirement,
        evidence: RuntimeVerifiedContentAvailability,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let _lock = ExclusiveFileLock::acquire(&self.lock_path)?;
        self.ensure_root_dir()?;
        let mut record = self.read_record(mint_id)?;
        if record.custody_terminal != Some(RuntimeCustodyTerminalKind::CustodyProvisioned)
            || !evidence.matches_requirement(requirement)
            || !evidence.matches_draft(&record.draft)
        {
            return Err(RuntimeMintJournalError::Conflict);
        }
        match &record.content_availability {
            Some(existing) if existing == &evidence => return Ok(record),
            Some(_) => return Err(RuntimeMintJournalError::Conflict),
            None => {}
        }
        record.content_availability = Some(evidence);
        self.write_replace(&record)?;
        Ok(record)
    }

    fn ensure_root_dir(&self) -> Result<(), RuntimeMintJournalError> {
        if let Some(parent) = self.root_dir.parent() {
            create_owner_only_directory(parent)?;
            validate_owner_only_directory(parent)?;
        }
        create_owner_only_directory(&self.root_dir)?;
        validate_owner_only_directory(&self.root_dir)
    }

    fn record_path(&self, mint_id: Digest32) -> PathBuf {
        self.root_dir.join(hex::encode(mint_id.as_bytes()))
    }

    fn temp_path(&self, mint_id: Digest32) -> PathBuf {
        self.root_dir
            .join(format!("{}.tmp", hex::encode(mint_id.as_bytes())))
    }

    fn intent_path(&self, request_id: Digest32) -> PathBuf {
        self.root_dir
            .join(format!("intent-{}", hex::encode(request_id.as_bytes())))
    }

    fn intent_temp_path(&self, request_id: Digest32) -> PathBuf {
        self.root_dir
            .join(format!("intent-{}.tmp", hex::encode(request_id.as_bytes())))
    }

    fn write_or_replay(
        &self,
        record: &PersistedRuntimeMint,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let path = self.record_path(record.draft.mint_id);
        if path.exists() {
            let existing = self.read_record(record.draft.mint_id)?;
            if existing.draft != record.draft {
                return Err(RuntimeMintJournalError::Conflict);
            }
            return Ok(existing);
        }
        self.write_replace(record)?;
        Ok(record.clone())
    }

    fn write_replace(&self, record: &PersistedRuntimeMint) -> Result<(), RuntimeMintJournalError> {
        let bytes = encode_record(record)?;
        let temp_path = self.temp_path(record.draft.mint_id);
        let _ = fs::remove_file(&temp_path);
        let mut temp_file = open_owner_only_temp_file_for_write(&temp_path)?;
        temp_file
            .write_all(&bytes)
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        temp_file
            .sync_all()
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        fs::rename(&temp_path, self.record_path(record.draft.mint_id))
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        sync_directory(&self.root_dir)
    }

    fn write_or_replay_intent(
        &self,
        intent: &RuntimeMintIntent,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let path = self.intent_path(intent.request_id);
        if path.exists() {
            let existing = self.read_intent(intent.request_id)?;
            if existing != *intent {
                return Err(RuntimeMintJournalError::Conflict);
            }
            return Ok(existing);
        }
        self.write_replace_intent(intent)?;
        Ok(intent.clone())
    }

    fn write_replace_intent(
        &self,
        intent: &RuntimeMintIntent,
    ) -> Result<(), RuntimeMintJournalError> {
        let bytes = encode_intent(intent)?;
        let temp_path = self.intent_temp_path(intent.request_id);
        let _ = fs::remove_file(&temp_path);
        let mut temp_file = open_owner_only_temp_file_for_write(&temp_path)?;
        temp_file
            .write_all(&bytes)
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        temp_file
            .sync_all()
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        fs::rename(&temp_path, self.intent_path(intent.request_id))
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        sync_directory(&self.root_dir)
    }

    fn read_record(
        &self,
        mint_id: Digest32,
    ) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
        let path = self.record_path(mint_id);
        let mut file = open_owner_only_file_for_read(&path).map_err(|error| {
            if !path.exists() {
                RuntimeMintJournalError::NotFound
            } else {
                error
            }
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        if bytes.len() > MAX_STORE_FILE_BYTES {
            return Err(RuntimeMintJournalError::Corrupt);
        }
        let record = decode_record(&bytes)?;
        if record.draft.mint_id != mint_id {
            return Err(RuntimeMintJournalError::Corrupt);
        }
        Ok(record)
    }

    fn read_intent(
        &self,
        request_id: Digest32,
    ) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
        let path = self.intent_path(request_id);
        let mut file = open_owner_only_file_for_read(&path).map_err(|error| {
            if !path.exists() {
                RuntimeMintJournalError::NotFound
            } else {
                error
            }
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        if bytes.len() > MAX_STORE_FILE_BYTES {
            return Err(RuntimeMintJournalError::Corrupt);
        }
        let intent = decode_intent(&bytes)?;
        if intent.request_id != request_id {
            return Err(RuntimeMintJournalError::Corrupt);
        }
        Ok(intent)
    }
}

fn encode_record(record: &PersistedRuntimeMint) -> Result<Vec<u8>, RuntimeMintJournalError> {
    let mut payload = Vec::new();
    push_digest(&mut payload, record.draft.mint_id);
    push_nested(
        &mut payload,
        &record
            .draft
            .media_identity
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    )?;
    payload.extend_from_slice(record.draft.content_access_id.as_bytes());
    push_nested(
        &mut payload,
        &record
            .draft
            .key_envelope
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    )?;
    push_nested(
        &mut payload,
        &record
            .draft
            .policy
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    )?;
    push_digest(&mut payload, record.draft.content_key_commitment);
    payload.push(record.draft.threshold.required());
    payload.push(record.draft.threshold.total());
    payload.push(
        u8::try_from(record.node_states.len()).map_err(|_| RuntimeMintJournalError::Corrupt)?,
    );
    for state in &record.node_states {
        payload.extend_from_slice(state.binding.node_public_key.as_bytes());
        payload.extend_from_slice(state.binding.operator_id.as_bytes());
        payload.extend_from_slice(state.binding.failure_domain_id.as_bytes());
        payload.extend_from_slice(state.binding.owner_state_root.as_bytes());
        payload.push(u8::from(state.effect_started));
        match &state.receipt {
            None => payload.push(0),
            Some(receipt) => {
                payload.push(1);
                payload.extend_from_slice(receipt.provisioning_id.digest().as_bytes());
                payload.extend_from_slice(receipt.record_identity.record_sha256().as_bytes());
                payload.extend_from_slice(&receipt.record_identity.record_bytes().to_be_bytes());
            }
        }
    }
    payload.push(match record.custody_terminal {
        None => 0,
        Some(RuntimeCustodyTerminalKind::CustodyProvisioned) => 1,
        Some(RuntimeCustodyTerminalKind::AbortedPartialProvision) => 2,
    });
    match &record.content_availability {
        None => payload.push(0),
        Some(evidence) => {
            payload.push(1);
            push_availability_text(&mut payload, &evidence.content_cid)?;
            push_availability_text(&mut payload, &evidence.object_identity)?;
            push_availability_text(&mut payload, &evidence.publisher_identity)?;
            push_availability_text(&mut payload, &evidence.expected_provider_did)?;
            push_availability_text(&mut payload, &evidence.policy)?;
            payload.extend_from_slice(&evidence.required_replicas.to_be_bytes());
            payload.extend_from_slice(&evidence.observed_replicas.to_be_bytes());
            payload.extend_from_slice(&evidence.checked_at.to_be_bytes());
            payload.extend_from_slice(evidence.receipt_digest.as_bytes());
            push_nested(
                &mut payload,
                &evidence
                    .encrypted_content
                    .canonical_bytes()
                    .map_err(|_| RuntimeMintJournalError::Corrupt)?,
            )?;
            payload.extend_from_slice(evidence.media_manifest_root.as_bytes());
        }
    }
    let digest = {
        let mut hasher = Sha256::new();
        hasher.update(STORE_DIGEST_DOMAIN);
        hasher.update(&payload);
        hasher.finalize()
    };
    let mut out = Vec::with_capacity(8 + 32 + payload.len());
    out.extend_from_slice(STORE_MAGIC);
    out.extend_from_slice(&digest);
    out.extend_from_slice(&payload);
    Ok(out)
}

fn decode_record(bytes: &[u8]) -> Result<PersistedRuntimeMint, RuntimeMintJournalError> {
    if bytes.len() < 8 + 32 {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    if &bytes[..8] != STORE_MAGIC {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let expected = &bytes[8..40];
    let payload = &bytes[40..];
    let mut hasher = Sha256::new();
    hasher.update(STORE_DIGEST_DOMAIN);
    hasher.update(payload);
    if hasher.finalize().as_slice() != expected {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let mut off = 0;
    let mint_id = read_digest(payload, &mut off)?;
    let media_identity =
        CencFmp4MediaIdentityV1::from_canonical_bytes(&read_nested(payload, &mut off)?)
            .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let content_access_id = ContentAccessIdV1::new(read_len16(payload, &mut off)?)
        .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let key_envelope =
        KeyEnvelopeIdentityV1::from_canonical_bytes(&read_nested(payload, &mut off)?)
            .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let policy = RightsPolicyIdentityV1::from_canonical_bytes(&read_nested(payload, &mut off)?)
        .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let content_key_commitment = read_digest(payload, &mut off)?;
    let required = read_u8(payload, &mut off)?;
    let total = read_u8(payload, &mut off)?;
    let threshold =
        ThresholdV1::new(required, total).map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let node_count = usize::from(read_u8(payload, &mut off)?);
    let mut node_states = Vec::with_capacity(node_count);
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let node_public_key = NodePublicKey::new(read_fixed(payload, &mut off)?)
            .map_err(|_| RuntimeMintJournalError::Corrupt)?;
        let operator_id = CustodyPoolOperatorIdV1::new(read_fixed(payload, &mut off)?);
        let failure_domain_id = CustodyPoolFailureDomainIdV1::new(read_fixed(payload, &mut off)?);
        let owner_state_root = Digest32::new(read_fixed(payload, &mut off)?);
        let binding = RuntimeMintNodeBinding::new(
            node_public_key,
            operator_id,
            failure_domain_id,
            owner_state_root,
        )?;
        let effect_started = read_u8(payload, &mut off)? != 0;
        let has_receipt = read_u8(payload, &mut off)? != 0;
        let receipt = if has_receipt {
            let provisioning_id =
                RuntimeCustodyProvisioningIdV1::new(Digest32::new(read_fixed(payload, &mut off)?))
                    .map_err(|_| RuntimeMintJournalError::Corrupt)?;
            let record_sha = Digest32::new(read_fixed(payload, &mut off)?);
            let record_bytes = u32::from_be_bytes(read_len4(payload, &mut off)?);
            Some(RuntimeMintNodeReceipt::new(
                node_public_key,
                provisioning_id,
                CustodyNodeProvisioningRecordIdentityV1::new(record_sha, record_bytes)
                    .map_err(|_| RuntimeMintJournalError::Corrupt)?,
                owner_state_root,
            )?)
        } else {
            None
        };
        nodes.push(binding.clone());
        node_states.push(MintNodeState {
            binding,
            effect_started,
            receipt,
        });
    }
    let custody_terminal = match read_u8(payload, &mut off)? {
        0 => None,
        1 => Some(RuntimeCustodyTerminalKind::CustodyProvisioned),
        2 => Some(RuntimeCustodyTerminalKind::AbortedPartialProvision),
        _ => return Err(RuntimeMintJournalError::Corrupt),
    };
    let content_availability = match read_u8(payload, &mut off)? {
        0 => None,
        1 => {
            let content_cid = read_availability_text(payload, &mut off)?;
            let object_identity = read_availability_text(payload, &mut off)?;
            let publisher_identity = read_availability_text(payload, &mut off)?;
            let expected_provider_did = read_availability_text(payload, &mut off)?;
            let policy = read_availability_text(payload, &mut off)?;
            let required_replicas = read_u32(payload, &mut off)?;
            let observed_replicas = read_u32(payload, &mut off)?;
            let checked_at = read_u64(payload, &mut off)?;
            let receipt_digest = read_digest(payload, &mut off)?;
            let encrypted_content =
                EncryptedContentIdentityV1::from_canonical_bytes(&read_nested(payload, &mut off)?)
                    .map_err(|_| RuntimeMintJournalError::Corrupt)?;
            let media_manifest_root = read_digest(payload, &mut off)?;
            let evidence = RuntimeVerifiedContentAvailability {
                content_cid,
                object_identity,
                publisher_identity,
                expected_provider_did,
                policy,
                required_replicas,
                observed_replicas,
                checked_at,
                receipt_digest,
                encrypted_content,
                media_manifest_root,
            };
            evidence.validate()?;
            Some(evidence)
        }
        _ => return Err(RuntimeMintJournalError::Corrupt),
    };
    if off != payload.len() {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let draft = RuntimeMintDraft::new_recorded(
        media_identity,
        content_access_id,
        key_envelope,
        policy,
        content_key_commitment,
        threshold,
        nodes,
    )?;
    if draft.mint_id != mint_id {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    Ok(PersistedRuntimeMint {
        draft,
        node_states,
        custody_terminal,
        content_availability,
    })
}

fn encode_intent(intent: &RuntimeMintIntent) -> Result<Vec<u8>, RuntimeMintJournalError> {
    let mut payload = Vec::new();
    push_digest(&mut payload, intent.request_id);
    push_availability_text(&mut payload, &intent.principal_id)?;
    push_digest(&mut payload, intent.source_binding_digest);
    push_availability_text(&mut payload, &intent.mime_type)?;
    push_availability_text(&mut payload, &intent.codecs)?;
    push_digest(&mut payload, intent.clear_init_sha256);
    payload.extend_from_slice(
        &u32::try_from(intent.clear_segment_sha256.len())
            .map_err(|_| RuntimeMintJournalError::Corrupt)?
            .to_be_bytes(),
    );
    for digest in &intent.clear_segment_sha256 {
        push_digest(&mut payload, *digest);
    }
    payload.extend_from_slice(intent.content_access_id.as_bytes());
    match intent.protect_state {
        RuntimeMintIntentProtectState::NotStarted => payload.push(0),
        RuntimeMintIntentProtectState::OpenRequestPending => payload.push(1),
        RuntimeMintIntentProtectState::OpenHandlePendingCancel(handle) => {
            payload.push(2);
            payload.extend_from_slice(&handle);
        }
        RuntimeMintIntentProtectState::OpenHandlePendingClose(handle) => {
            payload.push(3);
            payload.extend_from_slice(&handle);
        }
        RuntimeMintIntentProtectState::SettledBeforeDraft(
            RuntimeMintIntentProtectSettlement::Cancelled,
        ) => payload.push(4),
        RuntimeMintIntentProtectState::SettledBeforeDraft(
            RuntimeMintIntentProtectSettlement::Closed,
        ) => payload.push(5),
        RuntimeMintIntentProtectState::SettledBeforeDraft(
            RuntimeMintIntentProtectSettlement::AlreadyAbsent,
        ) => payload.push(6),
        RuntimeMintIntentProtectState::Completed(mint_id) => {
            payload.push(7);
            push_digest(&mut payload, mint_id);
        }
    }
    push_nested(
        &mut payload,
        &intent
            .custody_pool
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    )?;
    push_nested(
        &mut payload,
        &intent
            .custody_epoch
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    )?;
    push_nested(
        &mut payload,
        &intent
            .custody_committee_authorization
            .canonical_bytes()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    )?;
    payload.push(u8::try_from(intent.nodes.len()).map_err(|_| RuntimeMintJournalError::Corrupt)?);
    for node in &intent.nodes {
        payload.extend_from_slice(node.node_public_key.as_bytes());
        payload.extend_from_slice(node.operator_id.as_bytes());
        payload.extend_from_slice(node.failure_domain_id.as_bytes());
        payload.extend_from_slice(node.owner_state_root.as_bytes());
    }
    let digest = {
        let mut hasher = Sha256::new();
        hasher.update(INTENT_DIGEST_DOMAIN);
        hasher.update(&payload);
        hasher.finalize()
    };
    let mut out = Vec::with_capacity(8 + 32 + payload.len());
    out.extend_from_slice(INTENT_MAGIC);
    out.extend_from_slice(&digest);
    out.extend_from_slice(&payload);
    Ok(out)
}

fn decode_intent(bytes: &[u8]) -> Result<RuntimeMintIntent, RuntimeMintJournalError> {
    if bytes.len() < 8 + 32 {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    if &bytes[..8] != INTENT_MAGIC {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let expected = &bytes[8..40];
    let payload = &bytes[40..];
    let mut hasher = Sha256::new();
    hasher.update(INTENT_DIGEST_DOMAIN);
    hasher.update(payload);
    if hasher.finalize().as_slice() != expected {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let mut off = 0;
    let request_id = read_digest(payload, &mut off)?;
    let principal_id = read_availability_text(payload, &mut off)?;
    let source_binding_digest = read_digest(payload, &mut off)?;
    let mime_type = read_availability_text(payload, &mut off)?;
    let codecs = read_availability_text(payload, &mut off)?;
    let clear_init_sha256 = read_digest(payload, &mut off)?;
    let clear_segment_count = usize::try_from(read_u32(payload, &mut off)?)
        .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let mut clear_segment_sha256 = Vec::with_capacity(clear_segment_count);
    for _ in 0..clear_segment_count {
        clear_segment_sha256.push(read_digest(payload, &mut off)?);
    }
    let content_access_id = ContentAccessIdV1::new(read_len16(payload, &mut off)?)
        .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let protect_state = match read_u8(payload, &mut off)? {
        0 => RuntimeMintIntentProtectState::NotStarted,
        1 => RuntimeMintIntentProtectState::OpenRequestPending,
        2 => {
            RuntimeMintIntentProtectState::OpenHandlePendingCancel(read_handle(payload, &mut off)?)
        }
        3 => RuntimeMintIntentProtectState::OpenHandlePendingClose(read_handle(payload, &mut off)?),
        4 => RuntimeMintIntentProtectState::SettledBeforeDraft(
            RuntimeMintIntentProtectSettlement::Cancelled,
        ),
        5 => RuntimeMintIntentProtectState::SettledBeforeDraft(
            RuntimeMintIntentProtectSettlement::Closed,
        ),
        6 => RuntimeMintIntentProtectState::SettledBeforeDraft(
            RuntimeMintIntentProtectSettlement::AlreadyAbsent,
        ),
        7 => RuntimeMintIntentProtectState::Completed(read_digest(payload, &mut off)?),
        _ => return Err(RuntimeMintJournalError::Corrupt),
    };
    let custody_pool =
        CustodyPoolIdentityV1::from_canonical_bytes(&read_nested(payload, &mut off)?)
            .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let custody_epoch =
        CustodyEpochIdentityV1::from_canonical_bytes(&read_nested(payload, &mut off)?)
            .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let custody_committee_authorization =
        CustodyCommitteeAuthorizationIdentityV1::from_canonical_bytes(&read_nested(
            payload, &mut off,
        )?)
        .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    let node_count = usize::from(read_u8(payload, &mut off)?);
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        nodes.push(RuntimeMintNodeBinding::new(
            NodePublicKey::new(read_fixed(payload, &mut off)?)
                .map_err(|_| RuntimeMintJournalError::Corrupt)?,
            CustodyPoolOperatorIdV1::new(read_fixed(payload, &mut off)?),
            CustodyPoolFailureDomainIdV1::new(read_fixed(payload, &mut off)?),
            read_digest(payload, &mut off)?,
        )?);
    }
    if off != payload.len() {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let intent = RuntimeMintIntent {
        request_id,
        principal_id,
        source_binding_digest,
        mime_type,
        codecs,
        clear_init_sha256,
        clear_segment_sha256,
        content_access_id,
        protect_state,
        custody_pool,
        custody_epoch,
        custody_committee_authorization,
        nodes,
    };
    intent.validate()?;
    Ok(intent)
}

fn read_handle(
    bytes: &[u8],
    off: &mut usize,
) -> Result<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], RuntimeMintJournalError> {
    if bytes.len().saturating_sub(*off) < MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1 {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let mut handle = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
    handle.copy_from_slice(&bytes[*off..*off + MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]);
    *off += MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1;
    Ok(handle)
}

fn push_digest(payload: &mut Vec<u8>, digest: Digest32) {
    payload.extend_from_slice(digest.as_bytes());
}

fn push_nested(payload: &mut Vec<u8>, bytes: &[u8]) -> Result<(), RuntimeMintJournalError> {
    let len = u32::try_from(bytes.len()).map_err(|_| RuntimeMintJournalError::Corrupt)?;
    payload.extend_from_slice(&len.to_be_bytes());
    payload.extend_from_slice(bytes);
    Ok(())
}

fn push_availability_text(
    payload: &mut Vec<u8>,
    value: &str,
) -> Result<(), RuntimeMintJournalError> {
    let len = u16::try_from(value.len()).map_err(|_| RuntimeMintJournalError::Corrupt)?;
    payload.extend_from_slice(&len.to_be_bytes());
    payload.extend_from_slice(value.as_bytes());
    Ok(())
}

fn read_u8(payload: &[u8], off: &mut usize) -> Result<u8, RuntimeMintJournalError> {
    let value = *payload.get(*off).ok_or(RuntimeMintJournalError::Corrupt)?;
    *off += 1;
    Ok(value)
}

fn read_u32(payload: &[u8], off: &mut usize) -> Result<u32, RuntimeMintJournalError> {
    Ok(u32::from_be_bytes(read_len4(payload, off)?))
}

fn read_u64(payload: &[u8], off: &mut usize) -> Result<u64, RuntimeMintJournalError> {
    let end = off.checked_add(8).ok_or(RuntimeMintJournalError::Corrupt)?;
    let bytes = payload
        .get(*off..end)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    *off = end;
    Ok(u64::from_be_bytes(
        bytes
            .try_into()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    ))
}

fn read_availability_text(
    payload: &[u8],
    off: &mut usize,
) -> Result<String, RuntimeMintJournalError> {
    let end_len = off.checked_add(2).ok_or(RuntimeMintJournalError::Corrupt)?;
    let len = payload
        .get(*off..end_len)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    *off = end_len;
    let len = usize::from(u16::from_be_bytes(
        len.try_into()
            .map_err(|_| RuntimeMintJournalError::Corrupt)?,
    ));
    if len == 0 || len > MAX_AVAILABILITY_TEXT_BYTES {
        return Err(RuntimeMintJournalError::Corrupt);
    }
    let end = off
        .checked_add(len)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    let value = std::str::from_utf8(
        payload
            .get(*off..end)
            .ok_or(RuntimeMintJournalError::Corrupt)?,
    )
    .map_err(|_| RuntimeMintJournalError::Corrupt)?;
    *off = end;
    validate_availability_text(value).map_err(|_| RuntimeMintJournalError::Corrupt)?;
    Ok(value.to_string())
}

fn validate_availability_text(value: &str) -> Result<(), RuntimeMintJournalError> {
    if value.is_empty()
        || value.len() > MAX_AVAILABILITY_TEXT_BYTES
        || !value
            .as_bytes()
            .iter()
            .all(|byte| matches!(*byte, 0x21..=0x7e))
    {
        return Err(RuntimeMintJournalError::InvalidSelection);
    }
    Ok(())
}

fn validate_intent_text(value: &str) -> Result<(), RuntimeMintJournalError> {
    validate_availability_text(value)
}

fn compute_source_binding_digest(object_uri: &str, source_storage: &str) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(MINT_INTENT_SOURCE_BINDING_DOMAIN);
    hasher.update(object_uri.as_bytes());
    hasher.update([0u8]);
    hasher.update(source_storage.as_bytes());
    Digest32::new(hasher.finalize().into())
}

fn compute_mint_intent_request_id(principal_id: &str, source_binding_digest: Digest32) -> Digest32 {
    let mut hasher = Sha256::new();
    hasher.update(MINT_INTENT_REQUEST_ID_DOMAIN);
    hasher.update(principal_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(source_binding_digest.as_bytes());
    Digest32::new(hasher.finalize().into())
}

fn read_fixed(payload: &[u8], off: &mut usize) -> Result<[u8; 32], RuntimeMintJournalError> {
    let end = off
        .checked_add(32)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    let slice = payload
        .get(*off..end)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    *off = end;
    slice
        .try_into()
        .map_err(|_| RuntimeMintJournalError::Corrupt)
}

fn read_digest(payload: &[u8], off: &mut usize) -> Result<Digest32, RuntimeMintJournalError> {
    Ok(Digest32::new(read_fixed(payload, off)?))
}

fn read_len4(payload: &[u8], off: &mut usize) -> Result<[u8; 4], RuntimeMintJournalError> {
    let end = off.checked_add(4).ok_or(RuntimeMintJournalError::Corrupt)?;
    let slice = payload
        .get(*off..end)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    *off = end;
    slice
        .try_into()
        .map_err(|_| RuntimeMintJournalError::Corrupt)
}

fn read_len16(payload: &[u8], off: &mut usize) -> Result<[u8; 16], RuntimeMintJournalError> {
    let end = off
        .checked_add(16)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    let slice = payload
        .get(*off..end)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    *off = end;
    slice
        .try_into()
        .map_err(|_| RuntimeMintJournalError::Corrupt)
}

fn read_nested(payload: &[u8], off: &mut usize) -> Result<Vec<u8>, RuntimeMintJournalError> {
    let len = u32::from_be_bytes(read_len4(payload, off)?) as usize;
    let end = off
        .checked_add(len)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    let slice = payload
        .get(*off..end)
        .ok_or(RuntimeMintJournalError::Corrupt)?;
    *off = end;
    Ok(slice.to_vec())
}

struct ExclusiveFileLock {
    _lock: Flock<File>,
}

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> Result<Self, RuntimeMintJournalError> {
        if let Some(parent) = path.parent() {
            create_owner_only_directory(parent)?;
            validate_owner_only_directory(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            options.custom_flags(nix::libc::O_CLOEXEC);
        }
        let file = options
            .open(path)
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        let lock = Flock::lock(file, FlockArg::LockExclusive)
            .map_err(|_| RuntimeMintJournalError::Unavailable)?;
        Ok(Self { _lock: lock })
    }
}

fn create_owner_only_directory(path: &Path) -> Result<(), RuntimeMintJournalError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(RuntimeMintJournalError::Unavailable),
    }
}

fn validate_owner_only_directory(path: &Path) -> Result<(), RuntimeMintJournalError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeMintJournalError::Unavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RuntimeMintJournalError::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o777 != 0o700 {
            return Err(RuntimeMintJournalError::Unavailable);
        }
    }
    Ok(())
}

fn open_owner_only_file_for_read(path: &Path) -> Result<File, RuntimeMintJournalError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
    }
    options
        .open(path)
        .map_err(|_| RuntimeMintJournalError::Unavailable)
}

fn open_owner_only_temp_file_for_write(path: &Path) -> Result<File, RuntimeMintJournalError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
    }
    options
        .open(path)
        .map_err(|_| RuntimeMintJournalError::Unavailable)
}

fn sync_directory(path: &Path) -> Result<(), RuntimeMintJournalError> {
    let dir = File::open(path).map_err(|_| RuntimeMintJournalError::Unavailable)?;
    dir.sync_all()
        .map_err(|_| RuntimeMintJournalError::Unavailable)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use ed25519_dalek::SigningKey;
    use elastos_protected_content_contracts::{
        ContentAccessIdV1, CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIdentityV1,
        CustodyPoolIdentityV1, EncryptedContentIdentityV1, KeyEnvelopeIdentityV1, NodeSetV1,
        RightsPolicyIdentityV1, ThresholdV1,
    };
    use elastos_protected_content_provider_contracts::CencFmp4MediaIdentityV1;
    use tempfile::tempdir;

    use super::*;
    use crate::test_media;

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    fn content_access_id(seed: u8) -> ContentAccessIdV1 {
        ContentAccessIdV1::new([seed; 16]).unwrap()
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        NodePublicKey::new(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap()
    }

    fn binding(seed: u8) -> RuntimeMintNodeBinding {
        RuntimeMintNodeBinding::new(
            node_public_key(seed),
            CustodyPoolOperatorIdV1::new([0x80 + seed; 32]),
            CustodyPoolFailureDomainIdV1::new([0x90 + seed; 32]),
            digest(0xa0 + seed),
        )
        .unwrap()
    }

    fn nodes() -> Vec<RuntimeMintNodeBinding> {
        vec![binding(1), binding(2), binding(3)]
    }

    fn media_identity() -> CencFmp4MediaIdentityV1 {
        test_media::media_identity(0x21)
    }

    fn media_components() -> (Vec<u8>, Vec<Vec<u8>>, &'static str, &'static str) {
        test_media::media_components(0x21)
    }

    fn draft() -> RuntimeMintDraft {
        let nodes = nodes();
        let threshold = ThresholdV1::new(2, 3).unwrap();
        let media = media_identity();
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        let node_set = NodeSetV1::new(
            threshold,
            nodes.iter().map(|node| node.node_public_key()).collect(),
        )
        .unwrap();
        let key_envelope = KeyEnvelopeIdentityV1::new(
            media.encrypted_content().clone(),
            digest(0x22),
            512,
            node_set.node_set_id().unwrap(),
            threshold,
            CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
            CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
        )
        .unwrap();
        RuntimeMintDraft::new(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
            content_access_id(0x41),
            key_envelope,
            RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
            digest(0x19),
            threshold,
            nodes,
        )
        .unwrap()
    }

    fn intent() -> RuntimeMintIntent {
        let (init_segment, clear_segments, mime_type, codecs) = test_media::media_components(0x41);
        RuntimeMintIntent::new(
            "person:local:runtime-mint-intent-test",
            "localhost://Users/test/Documents/protected-clear-media",
            "plain_localhost_root",
            mime_type,
            codecs,
            &init_segment,
            &clear_segments,
            content_access_id(0x52),
            CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
            CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
            nodes(),
        )
        .unwrap()
    }

    fn receipt(node: &RuntimeMintNodeBinding, seed: u8) -> RuntimeMintNodeReceipt {
        RuntimeMintNodeReceipt::new(
            node.node_public_key(),
            RuntimeCustodyProvisioningIdV1::new(digest(seed)).unwrap(),
            CustodyNodeProvisioningRecordIdentityV1::new(digest(seed ^ 0x21), 128).unwrap(),
            node.owner_state_root(),
        )
        .unwrap()
    }

    fn owner_only_journal_root(temp: &tempfile::TempDir) -> PathBuf {
        let parent = temp.path().join("owner-only-parent");
        create_owner_only_directory(&parent).unwrap();
        parent.join("runtime-mint")
    }

    fn availability_requirement() -> RuntimeContentAvailabilityRequirement {
        RuntimeContentAvailabilityRequirement::new(
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#content",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#publisher",
            "protected-content-replication/v1",
            3,
            60,
            5,
        )
        .unwrap()
    }

    fn availability_evidence(
        draft: &RuntimeMintDraft,
        receipt_seed: u8,
    ) -> RuntimeVerifiedContentAvailability {
        RuntimeVerifiedContentAvailability::new(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#content",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#publisher",
            &availability_requirement(),
            3,
            2_000_000_000,
            digest(receipt_seed),
            draft.encrypted_content().clone(),
            draft.media_identity().media_manifest_root(),
        )
        .unwrap()
    }

    fn custody_provision_all(
        journal: &RuntimeMintJournal,
        draft: &RuntimeMintDraft,
    ) -> PersistedRuntimeMint {
        journal.persist_bound(draft).unwrap();
        for seed in [1u8, 2, 3] {
            let node = binding(seed);
            journal
                .mark_node_effect_started(draft.mint_id(), node.node_public_key())
                .unwrap();
            journal
                .mark_node_receipt(draft.mint_id(), receipt(&node, 0x80 + seed))
                .unwrap();
        }
        journal.mark_custody_provisioned(draft.mint_id()).unwrap()
    }

    #[test]
    fn one_node_and_duplicate_roots_fail_closed() {
        let media = media_identity();
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        let threshold = ThresholdV1::new(2, 3).unwrap();
        assert_eq!(
            RuntimeMintDraft::new(
                &init_segment,
                &encrypted_segments,
                mime_type,
                codecs,
                content_access_id(0x41),
                KeyEnvelopeIdentityV1::new(
                    media.encrypted_content().clone(),
                    digest(0x22),
                    512,
                    digest(0x23),
                    threshold,
                    CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
                    CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
                    CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
                )
                .unwrap(),
                RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
                digest(0x19),
                threshold,
                vec![binding(1)],
            )
            .unwrap_err(),
            RuntimeMintJournalError::InvalidSelection
        );

        let mut duplicate_roots = nodes();
        duplicate_roots[2] = RuntimeMintNodeBinding::new(
            node_public_key(3),
            duplicate_roots[2].operator_id(),
            duplicate_roots[2].failure_domain_id(),
            duplicate_roots[0].owner_state_root(),
        )
        .unwrap();
        let valid = draft();
        assert_eq!(
            RuntimeMintDraft::new(
                &init_segment,
                &encrypted_segments,
                mime_type,
                codecs,
                content_access_id(0x41),
                valid.key_envelope().clone(),
                valid.policy().clone(),
                valid.content_key_commitment(),
                valid.threshold(),
                duplicate_roots,
            )
            .unwrap_err(),
            RuntimeMintJournalError::InvalidSelection
        );

        let stale_payload_identity = EncryptedContentIdentityV1::new(
            digest(0xee),
            media.encrypted_content().ciphertext_bytes(),
        )
        .unwrap();
        assert_eq!(
            RuntimeMintDraft::new(
                &init_segment,
                &encrypted_segments,
                mime_type,
                codecs,
                content_access_id(0x41),
                KeyEnvelopeIdentityV1::new(
                    stale_payload_identity,
                    digest(0x22),
                    512,
                    digest(0x23),
                    threshold,
                    CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
                    CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
                    CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
                )
                .unwrap(),
                RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
                digest(0x19),
                threshold,
                nodes(),
            )
            .unwrap_err(),
            RuntimeMintJournalError::InvalidSelection
        );
    }

    #[test]
    fn media_segment_cap_stays_persistable_under_journal_limit() {
        let temp = tempdir().unwrap();
        let journal = RuntimeMintJournal::new(owner_only_journal_root(&temp));
        let threshold = ThresholdV1::new(2, 3).unwrap();
        let node_bindings = nodes();
        let (init_segment, encrypted_segments, mime_type, codecs) =
            test_media::media_components_with_segment_count(0x31, 512);
        let media = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
        )
        .unwrap();
        let node_set = NodeSetV1::new(
            threshold,
            node_bindings
                .iter()
                .map(|node| node.node_public_key())
                .collect(),
        )
        .unwrap();
        let key_envelope = KeyEnvelopeIdentityV1::new(
            media.encrypted_content().clone(),
            digest(0x22),
            512,
            node_set.node_set_id().unwrap(),
            threshold,
            CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
            CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
        )
        .unwrap();
        let draft = RuntimeMintDraft::new(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
            content_access_id(0x41),
            key_envelope,
            RightsPolicyIdentityV1::new(digest(0x44), 384).unwrap(),
            digest(0x19),
            threshold,
            node_bindings,
        )
        .unwrap();

        let persisted = journal.persist_bound(&draft).unwrap();
        assert_eq!(persisted.draft(), &draft);
        assert_eq!(journal.load(draft.mint_id()).unwrap().draft(), &draft);

        let (_, over_limit, _, _) = test_media::media_components_with_segment_count(0x31, 513);
        assert_eq!(
            RuntimeMintDraft::new(
                &init_segment,
                &over_limit,
                mime_type,
                codecs,
                draft.content_access_id(),
                draft.key_envelope().clone(),
                draft.policy().clone(),
                draft.content_key_commitment(),
                draft.threshold(),
                nodes(),
            )
            .unwrap_err(),
            RuntimeMintJournalError::InvalidSelection
        );
    }

    #[test]
    fn persist_before_effects_replays_exactly_and_never_provisions_partial_abort() {
        let temp = tempdir().unwrap();
        let journal = RuntimeMintJournal::new(owner_only_journal_root(&temp));
        let draft = draft();

        let persisted = journal.persist_bound(&draft).unwrap();
        assert!(persisted.custody_terminal().is_none());
        assert!(!persisted.any_effect_started());
        assert_eq!(
            journal.persist_bound(&draft).unwrap().draft(),
            persisted.draft()
        );

        journal
            .mark_node_effect_started(draft.mint_id(), node_public_key(1))
            .unwrap();
        journal
            .mark_node_receipt(draft.mint_id(), receipt(&binding(1), 0x81))
            .unwrap();
        journal
            .mark_node_effect_started(draft.mint_id(), node_public_key(2))
            .unwrap();
        let aborted = journal
            .mark_aborted_partial_provision(draft.mint_id())
            .unwrap();
        assert_eq!(
            aborted.custody_terminal(),
            Some(RuntimeCustodyTerminalKind::AbortedPartialProvision)
        );
        assert_eq!(aborted.accepted_orphans().len(), 1);
        assert_eq!(
            journal.mark_custody_provisioned(draft.mint_id()),
            Err(RuntimeMintJournalError::Conflict)
        );

        let reloaded = RuntimeMintJournal::new(owner_only_journal_root(&temp))
            .load(draft.mint_id())
            .unwrap();
        assert_eq!(reloaded.custody_terminal(), aborted.custody_terminal());
        assert_eq!(reloaded.accepted_orphans().len(), 1);
        assert_eq!(journal.persist_bound(&draft).unwrap(), reloaded);
    }

    #[test]
    fn mint_intent_protect_recovery_state_is_durable_and_exact_replay_safe() {
        let temp = tempdir().unwrap();
        let root = owner_only_journal_root(&temp);
        let journal = RuntimeMintJournal::new(&root);
        let intent = intent();
        let request_id = intent.request_id();
        let handle = [0x41; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        let mint_id = draft().mint_id();

        let persisted = journal.persist_intent(&intent).unwrap();
        assert_eq!(persisted.protect_state_label(), "not_started");
        assert!(!persisted.provider_effect_started());
        assert_eq!(journal.persist_intent(&intent).unwrap(), persisted);

        let pending = journal
            .mark_intent_protect_effect_started(request_id)
            .unwrap();
        assert!(pending.protect_open_request_pending());
        assert!(pending.provider_effect_started());
        assert_eq!(
            journal
                .mark_intent_protect_effect_started(request_id)
                .unwrap(),
            pending
        );

        let opened = journal
            .mark_intent_protect_opened(request_id, handle)
            .unwrap();
        assert_eq!(opened.protect_pending_cancel_handle(), Some(handle));
        assert_eq!(
            journal
                .mark_intent_protect_opened(request_id, handle)
                .unwrap(),
            opened
        );

        let finalized = journal
            .mark_intent_protect_finalized(request_id, handle)
            .unwrap();
        assert_eq!(finalized.protect_pending_close_handle(), Some(handle));
        assert_eq!(
            journal
                .mark_intent_protect_finalized(request_id, handle)
                .unwrap(),
            finalized
        );

        let settled = journal
            .mark_intent_protect_closed_before_draft(request_id)
            .unwrap();
        assert!(settled.protect_terminal_before_draft());
        assert_eq!(settled.protect_terminal_settlement_label(), Some("closed"));

        let completed = journal.mark_intent_completed(request_id, mint_id).unwrap();
        assert_eq!(completed.completed_mint_id(), Some(mint_id));
        assert_eq!(
            journal.mark_intent_completed(request_id, mint_id).unwrap(),
            completed
        );

        let reloaded = RuntimeMintJournal::new(&root)
            .load_intent(request_id)
            .unwrap();
        assert_eq!(reloaded, completed);
    }

    #[test]
    fn mint_intent_rejects_invalid_recovery_transitions() {
        let temp = tempdir().unwrap();
        let journal = RuntimeMintJournal::new(owner_only_journal_root(&temp));
        let intent = intent();
        journal.persist_intent(&intent).unwrap();
        let handle = [0x42; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        assert_eq!(
            journal.mark_intent_protect_closed_before_draft(intent.request_id()),
            Err(RuntimeMintJournalError::Conflict)
        );
        assert_eq!(
            journal.mark_intent_protect_opened(intent.request_id(), handle),
            Err(RuntimeMintJournalError::Conflict)
        );
        assert_eq!(
            journal.mark_intent_protect_finalized(intent.request_id(), handle),
            Err(RuntimeMintJournalError::Conflict)
        );
    }

    #[test]
    fn custody_provisioning_is_durable_and_identity_only() {
        let temp = tempdir().unwrap();
        let root = owner_only_journal_root(&temp);
        let journal = RuntimeMintJournal::new(&root);
        let draft = draft();
        journal.persist_bound(&draft).unwrap();
        for seed in [1u8, 2, 3] {
            let node = binding(seed);
            journal
                .mark_node_effect_started(draft.mint_id(), node.node_public_key())
                .unwrap();
            journal
                .mark_node_receipt(draft.mint_id(), receipt(&node, 0x80 + seed))
                .unwrap();
        }
        let provisioned = journal.mark_custody_provisioned(draft.mint_id()).unwrap();
        assert_eq!(
            provisioned.custody_terminal(),
            Some(RuntimeCustodyTerminalKind::CustodyProvisioned)
        );
        assert_eq!(
            journal.mark_custody_provisioned(draft.mint_id()).unwrap(),
            provisioned
        );

        let record_path = root.join(hex::encode(draft.mint_id().as_bytes()));
        let bytes = fs::read(&record_path).unwrap();
        assert!(bytes.len() < 16 * 1024);
        let debug = format!("{provisioned:?}");
        assert!(!debug.contains("sealed"));
        assert!(!debug.contains("/tmp/"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&record_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dir_mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
    }

    #[test]
    fn signed_content_availability_is_exact_idempotent_and_restart_safe() {
        let temp = tempdir().unwrap();
        let root = owner_only_journal_root(&temp);
        let journal = RuntimeMintJournal::new(&root);
        let draft = draft();
        custody_provision_all(&journal, &draft);

        let requirement = availability_requirement();
        let evidence = availability_evidence(&draft, 0x71);
        let available = journal
            .mark_content_available(draft.mint_id(), &requirement, evidence.clone())
            .unwrap();
        assert_eq!(available.content_availability(), Some(&evidence));
        assert_eq!(
            journal
                .mark_content_available(draft.mint_id(), &requirement, evidence.clone())
                .unwrap(),
            available
        );
        assert_eq!(
            journal.mark_content_available(
                draft.mint_id(),
                &requirement,
                availability_evidence(&draft, 0x72),
            ),
            Err(RuntimeMintJournalError::Conflict)
        );
        let reloaded = RuntimeMintJournal::new(&root)
            .load(draft.mint_id())
            .unwrap();
        assert_eq!(reloaded.content_availability(), Some(&evidence));
    }

    #[test]
    fn content_availability_never_converts_an_aborted_or_mismatched_mint() {
        let temp = tempdir().unwrap();
        let journal = RuntimeMintJournal::new(owner_only_journal_root(&temp));
        let first_draft = draft();
        journal.persist_bound(&first_draft).unwrap();
        let aborted = journal
            .mark_aborted_partial_provision(first_draft.mint_id())
            .unwrap();
        assert_eq!(
            journal.mark_content_available(
                first_draft.mint_id(),
                &availability_requirement(),
                availability_evidence(&first_draft, 0x71),
            ),
            Err(RuntimeMintJournalError::Conflict)
        );
        assert!(aborted.content_availability().is_none());

        let second_temp = tempdir().unwrap();
        let second_journal = RuntimeMintJournal::new(owner_only_journal_root(&second_temp));
        let second = draft();
        custody_provision_all(&second_journal, &second);
        let mut wrong_evidence = availability_evidence(&second, 0x73);
        wrong_evidence.media_manifest_root = digest(0x74);
        assert_eq!(
            second_journal.mark_content_available(
                second.mint_id(),
                &availability_requirement(),
                wrong_evidence,
            ),
            Err(RuntimeMintJournalError::Conflict)
        );
        let mut wrong_object = availability_evidence(&second, 0x75);
        wrong_object.object_identity = "did:key:wrong#content".to_string();
        assert_eq!(
            second_journal.mark_content_available(
                second.mint_id(),
                &availability_requirement(),
                wrong_object,
            ),
            Err(RuntimeMintJournalError::Conflict)
        );
        let mut wrong_publisher = availability_evidence(&second, 0x76);
        wrong_publisher.publisher_identity = "did:key:wrong#publisher".to_string();
        assert_eq!(
            second_journal.mark_content_available(
                second.mint_id(),
                &availability_requirement(),
                wrong_publisher,
            ),
            Err(RuntimeMintJournalError::Conflict)
        );
    }
}
