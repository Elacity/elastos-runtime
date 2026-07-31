//! Transport-independent Wallet Bus v2 contract.
//!
//! This crate owns only the private Wallet request and response schema. Runtime
//! verifies the launch/session authority before constructing
//! [`VerifiedWalletInvocationContext`]; Wallet owns account, proof, approval,
//! signing, and managed Recovery Key semantics. The schema contains no
//! transport, provider implementation, chain broadcast, or receipt behavior.
//! Its deterministic hashes bind request lifecycle data for integrity and
//! correlation; they do not authenticate a caller. Authority comes from the
//! authenticated Runtime-local provider plane.

#![forbid(unsafe_code)]

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;

pub const WALLET_PROTOCOL_VERSION: &str = "2.1";
pub const WALLET_BUS_OPERATION: &str = "wallet_contract";
pub const WALLET_REQUEST_SCHEMA: &str = "elastos.wallet.provider-request/v2";
pub const WALLET_RESPONSE_SCHEMA: &str = "elastos.wallet.provider-response/v2";
pub const ERC1271_EVIDENCE_SCHEMA: &str = "elastos.chain.erc1271_proof/v1";
pub const MANAGED_RECOVERY_SET_SCHEMA: &str = "elastos.wallet.managed-recovery-set/v1";
pub const DEFAULT_BITCOIN_NETWORK: &str = "bitcoin";
pub const MAX_INVOCATION_TTL_SECS: u64 = 300;
pub const MAX_CLOCK_SKEW_SECS: u64 = 60;
pub const MAX_APPROVAL_PAYLOAD_BYTES: usize = 32 * 1024;
pub const MAX_RECOVERY_KEY_BYTES: usize = 64 * 1024;
pub const MAX_MANAGED_RECOVERY_SET_KEYS: usize = 64;
pub const MAX_MANAGED_RECOVERY_SET_BYTES: usize = 256 * 1024;

const REQUEST_ID_PREFIX: &str = "wallet-request:";
const ERC1271_MAGIC_VALUE: &str = "0x1626ba7e";
const MAX_SIGNED_MESSAGE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(String);

impl ContractError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}

impl From<serde_json::Error> for ContractError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(format!("invalid Wallet Bus JSON: {error}"))
    }
}

pub type ContractResult<T> = Result<T, ContractError>;

/// A public Chain network identifier after syntax validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PublicNetwork(String);

impl PublicNetwork {
    pub fn new(value: impl Into<String>) -> ContractResult<Self> {
        let value = value.into();
        validate_token("public network", &value, 128)?;
        Ok(Self(value))
    }

    pub fn bitcoin() -> Self {
        Self(DEFAULT_BITCOIN_NETWORK.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PublicNetwork {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

fn default_bitcoin_network() -> PublicNetwork {
    PublicNetwork::bitcoin()
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Runtime authority that exists only after signed launch and session checks.
///
/// Its fields are private so callers cannot deserialize unverified JSON into a
/// value that claims to be verified. Runtime supplies this value to the request
/// constructor; the serialized request carries the derived public authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWalletInvocationContext {
    principal_id: String,
    session_id: String,
    proof_binding_id: Option<String>,
    grant_id: String,
    actor: String,
    launch_id: String,
}

impl VerifiedWalletInvocationContext {
    pub fn new(
        principal_id: impl Into<String>,
        session_id: impl Into<String>,
        proof_binding_id: Option<String>,
        grant_id: impl Into<String>,
        actor: impl Into<String>,
        launch_id: impl Into<String>,
    ) -> ContractResult<Self> {
        let context = Self {
            principal_id: principal_id.into(),
            session_id: session_id.into(),
            proof_binding_id,
            grant_id: grant_id.into(),
            actor: actor.into(),
            launch_id: launch_id.into(),
        };
        context.validate()?;
        Ok(context)
    }

    fn validate(&self) -> ContractResult<()> {
        validate_required("principal_id", &self.principal_id, 256)?;
        validate_required("session_id", &self.session_id, 256)?;
        validate_optional("proof_binding_id", self.proof_binding_id.as_deref(), 256)?;
        validate_required("grant_id", &self.grant_id, 256)?;
        validate_token("actor", &self.actor, 128)?;
        validate_required("launch_id", &self.launch_id, 256)
    }

    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn proof_binding_id(&self) -> Option<&str> {
        self.proof_binding_id.as_deref()
    }

    pub fn grant_id(&self) -> &str {
        &self.grant_id
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn launch_id(&self) -> &str {
        &self.launch_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletAuthorityV2 {
    pub principal_id: String,
    pub session_id: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub proof_binding_id: Option<String>,
    pub grant_id: String,
    pub actor: String,
    pub launch_id: String,
    pub capability: String,
    pub intent: String,
}

impl WalletAuthorityV2 {
    fn derive(
        context: &VerifiedWalletInvocationContext,
        operation: &WalletProviderOperationV2,
    ) -> Self {
        Self {
            principal_id: context.principal_id.clone(),
            session_id: context.session_id.clone(),
            proof_binding_id: context.proof_binding_id.clone(),
            grant_id: context.grant_id.clone(),
            actor: context.actor.clone(),
            launch_id: context.launch_id.clone(),
            capability: operation.capability().to_string(),
            intent: operation.authority_intent().to_string(),
        }
    }

    fn validate(&self) -> ContractResult<()> {
        validate_required("principal_id", &self.principal_id, 256)?;
        validate_required("session_id", &self.session_id, 256)?;
        validate_optional("proof_binding_id", self.proof_binding_id.as_deref(), 256)?;
        validate_required("grant_id", &self.grant_id, 256)?;
        validate_token("actor", &self.actor, 128)?;
        validate_required("launch_id", &self.launch_id, 256)?;
        validate_token("capability", &self.capability, 128)?;
        validate_token("intent", &self.intent, 128)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletOperationKind {
    ListAccounts,
    CreateManagedAccount,
    LinkVerifiedAccount,
    RevokeAccount,
    RenameAccount,
    SetDefaultAccount,
    DefaultAccount,
    Challenge,
    BitcoinChallenge,
    VerifyProof,
    VerifyContractProof,
    VerifyBip322Proof,
    RequestApproval,
    ListApprovals,
    RejectApproval,
    ApproveAndSignManaged,
    ApproveConnectorHandoff,
    CompleteConnectorHandoff,
    ExportManagedRecoveryKey,
    ImportManagedRecoveryKey,
    ExportManagedRecoverySet,
    ImportManagedRecoverySet,
}

impl WalletOperationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ListAccounts => "list_accounts",
            Self::CreateManagedAccount => "create_managed_account",
            Self::LinkVerifiedAccount => "link_verified_account",
            Self::RevokeAccount => "revoke_account",
            Self::RenameAccount => "rename_account",
            Self::SetDefaultAccount => "set_default_account",
            Self::DefaultAccount => "default_account",
            Self::Challenge => "challenge",
            Self::BitcoinChallenge => "bitcoin_challenge",
            Self::VerifyProof => "verify_proof",
            Self::VerifyContractProof => "verify_contract_proof",
            Self::VerifyBip322Proof => "verify_bip322_proof",
            Self::RequestApproval => "request_approval",
            Self::ListApprovals => "list_approvals",
            Self::RejectApproval => "reject_approval",
            Self::ApproveAndSignManaged => "approve_and_sign_managed",
            Self::ApproveConnectorHandoff => "approve_connector_handoff",
            Self::CompleteConnectorHandoff => "complete_connector_handoff",
            Self::ExportManagedRecoveryKey => "export_managed_recovery_key",
            Self::ImportManagedRecoveryKey => "import_managed_recovery_key",
            Self::ExportManagedRecoverySet => "export_managed_recovery_set",
            Self::ImportManagedRecoverySet => "import_managed_recovery_set",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletOperationClass {
    Read,
    Effectful,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Erc1271ProofEvidenceV1 {
    pub schema: String,
    pub network: PublicNetwork,
    pub chain_id: u64,
    pub contract: String,
    pub message_hash: String,
    pub signature_hash: String,
    pub valid: bool,
    pub magic_value: String,
    pub checked_at: u64,
}

impl Erc1271ProofEvidenceV1 {
    pub fn validate(&self) -> ContractResult<()> {
        if self.schema != ERC1271_EVIDENCE_SCHEMA {
            return Err(ContractError::new("unsupported ERC-1271 evidence schema"));
        }
        if self.chain_id == 0 {
            return Err(ContractError::new("ERC-1271 evidence chain_id is required"));
        }
        validate_evm_address("ERC-1271 contract", &self.contract)?;
        validate_hash32("ERC-1271 message_hash", &self.message_hash)?;
        validate_hash32("ERC-1271 signature_hash", &self.signature_hash)?;
        if !self.valid {
            return Err(ContractError::new("ERC-1271 evidence is not valid"));
        }
        if self.magic_value.to_ascii_lowercase() != ERC1271_MAGIC_VALUE {
            return Err(ContractError::new(
                "ERC-1271 evidence magic value is invalid",
            ));
        }
        if self.checked_at == 0 {
            return Err(ContractError::new(
                "ERC-1271 evidence checked_at is required",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedRecoveryKeyEntryV1 {
    pub account_id: String,
    pub recovery_key: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ManagedRecoveryKeyEntryV1 {
    pub fn validate(&self) -> ContractResult<()> {
        validate_required("managed recovery account_id", &self.account_id, 256)?;
        if !self.recovery_key.is_object() {
            return Err(ContractError::new(
                "managed recovery key must be a non-null JSON object",
            ));
        }
        validate_json_size(
            "managed recovery key",
            &self.recovery_key,
            MAX_RECOVERY_KEY_BYTES,
        )?;
        validate_optional("managed recovery label", self.label.as_deref(), 256)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedRecoverySetV1 {
    pub schema: String,
    pub keys: Vec<ManagedRecoveryKeyEntryV1>,
}

impl ManagedRecoverySetV1 {
    pub fn new(keys: Vec<ManagedRecoveryKeyEntryV1>) -> ContractResult<Self> {
        let recovery_set = Self {
            schema: MANAGED_RECOVERY_SET_SCHEMA.to_string(),
            keys,
        };
        recovery_set.validate()?;
        Ok(recovery_set)
    }

    pub fn validate(&self) -> ContractResult<()> {
        if self.schema != MANAGED_RECOVERY_SET_SCHEMA {
            return Err(ContractError::new(
                "unsupported managed recovery set schema",
            ));
        }
        if self.keys.len() > MAX_MANAGED_RECOVERY_SET_KEYS {
            return Err(ContractError::new(format!(
                "managed recovery set exceeds {MAX_MANAGED_RECOVERY_SET_KEYS} keys"
            )));
        }
        let mut account_ids = BTreeSet::new();
        for key in &self.keys {
            key.validate()?;
            if !account_ids.insert(key.account_id.as_str()) {
                return Err(ContractError::new(
                    "managed recovery set contains duplicate account entries",
                ));
            }
        }
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_MANAGED_RECOVERY_SET_BYTES {
            return Err(ContractError::new(format!(
                "managed recovery set exceeds {MAX_MANAGED_RECOVERY_SET_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WalletProviderOperationV2 {
    ListAccounts {
        include_revoked: bool,
    },
    CreateManagedAccount {
        chain_namespace: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        create_new: bool,
    },
    LinkVerifiedAccount {
        proof_binding_id: String,
        chain_namespace: String,
        address: String,
        proof_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    RevokeAccount {
        account_id: String,
    },
    RenameAccount {
        account_id: String,
        label: String,
    },
    SetDefaultAccount {
        chain_namespace: String,
        intent: String,
        account_id: String,
    },
    DefaultAccount {
        chain_namespace: String,
        intent: String,
    },
    Challenge {
        domain: String,
        uri: String,
        address: String,
        chain_id: u64,
        resources: Vec<String>,
    },
    BitcoinChallenge {
        domain: String,
        uri: String,
        address: String,
        #[serde(default = "default_bitcoin_network")]
        network: PublicNetwork,
        resources: Vec<String>,
    },
    VerifyProof {
        message: String,
        signature: String,
    },
    VerifyContractProof {
        message: String,
        signature: String,
        evidence: Erc1271ProofEvidenceV1,
    },
    VerifyBip322Proof {
        message: String,
        signature: String,
        signature_type: String,
        public_key: Option<String>,
    },
    RequestApproval {
        account_id: String,
        chain_namespace: String,
        intent: String,
        resource: String,
        reason: String,
        payload: Value,
        expires_at: u64,
    },
    ListApprovals {
        include_resolved: bool,
    },
    RejectApproval {
        request_id: String,
        reason: String,
    },
    ApproveAndSignManaged {
        request_id: String,
        reason: String,
    },
    ApproveConnectorHandoff {
        request_id: String,
        reason: String,
    },
    CompleteConnectorHandoff {
        request_id: String,
        payload_hash: String,
        signature: Option<String>,
        signature_type: Option<String>,
        public_key: Option<String>,
        signer: String,
        transaction_hash: Option<String>,
    },
    ExportManagedRecoveryKey {
        account_id: String,
    },
    ImportManagedRecoveryKey {
        recovery_key: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    ExportManagedRecoverySet {},
    ImportManagedRecoverySet {
        recovery_set: ManagedRecoverySetV1,
    },
}

impl WalletProviderOperationV2 {
    pub const fn kind(&self) -> WalletOperationKind {
        match self {
            Self::ListAccounts { .. } => WalletOperationKind::ListAccounts,
            Self::CreateManagedAccount { .. } => WalletOperationKind::CreateManagedAccount,
            Self::LinkVerifiedAccount { .. } => WalletOperationKind::LinkVerifiedAccount,
            Self::RevokeAccount { .. } => WalletOperationKind::RevokeAccount,
            Self::RenameAccount { .. } => WalletOperationKind::RenameAccount,
            Self::SetDefaultAccount { .. } => WalletOperationKind::SetDefaultAccount,
            Self::DefaultAccount { .. } => WalletOperationKind::DefaultAccount,
            Self::Challenge { .. } => WalletOperationKind::Challenge,
            Self::BitcoinChallenge { .. } => WalletOperationKind::BitcoinChallenge,
            Self::VerifyProof { .. } => WalletOperationKind::VerifyProof,
            Self::VerifyContractProof { .. } => WalletOperationKind::VerifyContractProof,
            Self::VerifyBip322Proof { .. } => WalletOperationKind::VerifyBip322Proof,
            Self::RequestApproval { .. } => WalletOperationKind::RequestApproval,
            Self::ListApprovals { .. } => WalletOperationKind::ListApprovals,
            Self::RejectApproval { .. } => WalletOperationKind::RejectApproval,
            Self::ApproveAndSignManaged { .. } => WalletOperationKind::ApproveAndSignManaged,
            Self::ApproveConnectorHandoff { .. } => WalletOperationKind::ApproveConnectorHandoff,
            Self::CompleteConnectorHandoff { .. } => WalletOperationKind::CompleteConnectorHandoff,
            Self::ExportManagedRecoveryKey { .. } => WalletOperationKind::ExportManagedRecoveryKey,
            Self::ImportManagedRecoveryKey { .. } => WalletOperationKind::ImportManagedRecoveryKey,
            Self::ExportManagedRecoverySet { .. } => WalletOperationKind::ExportManagedRecoverySet,
            Self::ImportManagedRecoverySet { .. } => WalletOperationKind::ImportManagedRecoverySet,
        }
    }

    pub const fn class(&self) -> WalletOperationClass {
        match self {
            Self::ListAccounts { .. }
            | Self::DefaultAccount { .. }
            | Self::ListApprovals { .. } => WalletOperationClass::Read,
            _ => WalletOperationClass::Effectful,
        }
    }

    pub const fn is_effectful(&self) -> bool {
        matches!(self.class(), WalletOperationClass::Effectful)
    }

    pub const fn capability(&self) -> &'static str {
        match self {
            Self::ListAccounts { .. } | Self::DefaultAccount { .. } => "wallet:accounts:read",
            Self::CreateManagedAccount { .. }
            | Self::LinkVerifiedAccount { .. }
            | Self::RevokeAccount { .. }
            | Self::RenameAccount { .. }
            | Self::SetDefaultAccount { .. } => "wallet:accounts:write",
            Self::Challenge { .. } | Self::BitcoinChallenge { .. } => "wallet:proof:challenge",
            Self::VerifyProof { .. }
            | Self::VerifyContractProof { .. }
            | Self::VerifyBip322Proof { .. } => "wallet:proof:verify",
            Self::RequestApproval { .. } => "wallet:approval:request",
            Self::ListApprovals { .. } => "wallet:approval:read",
            Self::RejectApproval { .. } => "wallet:approval:reject",
            Self::ApproveAndSignManaged { .. } => "wallet:approval:managed-sign",
            Self::ApproveConnectorHandoff { .. } => "wallet:approval:connector-approve",
            Self::CompleteConnectorHandoff { .. } => "wallet:approval:connector-complete",
            Self::ExportManagedRecoveryKey { .. } => "wallet:recovery:export-managed",
            Self::ImportManagedRecoveryKey { .. } => "wallet:recovery:import-managed",
            Self::ExportManagedRecoverySet { .. } => "wallet:recovery:export-managed-set",
            Self::ImportManagedRecoverySet { .. } => "wallet:recovery:import-managed-set",
        }
    }

    pub const fn authority_intent(&self) -> &'static str {
        match self {
            Self::ListAccounts { .. } => "wallet.accounts.list",
            Self::CreateManagedAccount { .. } => "wallet.account.create-managed",
            Self::LinkVerifiedAccount { .. } => "wallet.account.link-verified",
            Self::RevokeAccount { .. } => "wallet.account.revoke",
            Self::RenameAccount { .. } => "wallet.account.rename",
            Self::SetDefaultAccount { .. } => "wallet.account.default.set",
            Self::DefaultAccount { .. } => "wallet.account.default.read",
            Self::Challenge { .. } => "wallet.proof.evm.challenge",
            Self::BitcoinChallenge { .. } => "wallet.proof.bitcoin.challenge",
            Self::VerifyProof { .. } => "wallet.proof.evm.verify-eoa",
            Self::VerifyContractProof { .. } => "wallet.proof.evm.verify-contract",
            Self::VerifyBip322Proof { .. } => "wallet.proof.bitcoin.verify",
            Self::RequestApproval { .. } => "wallet.approval.request",
            Self::ListApprovals { .. } => "wallet.approval.list",
            Self::RejectApproval { .. } => "wallet.approval.reject",
            Self::ApproveAndSignManaged { .. } => "wallet.approval.managed.approve-sign",
            Self::ApproveConnectorHandoff { .. } => "wallet.approval.connector.approve",
            Self::CompleteConnectorHandoff { .. } => "wallet.approval.connector.complete",
            Self::ExportManagedRecoveryKey { .. } => "wallet.recovery.managed.export",
            Self::ImportManagedRecoveryKey { .. } => "wallet.recovery.managed.import",
            Self::ExportManagedRecoverySet { .. } => "wallet.recovery.managed-set.export",
            Self::ImportManagedRecoverySet { .. } => "wallet.recovery.managed-set.import",
        }
    }

    pub fn account_id(&self) -> Option<&str> {
        match self {
            Self::RevokeAccount { account_id }
            | Self::RenameAccount { account_id, .. }
            | Self::SetDefaultAccount { account_id, .. }
            | Self::RequestApproval { account_id, .. }
            | Self::ExportManagedRecoveryKey { account_id } => Some(account_id),
            _ => None,
        }
    }

    pub fn approval_request_id(&self) -> Option<&str> {
        match self {
            Self::RejectApproval { request_id, .. }
            | Self::ApproveAndSignManaged { request_id, .. }
            | Self::ApproveConnectorHandoff { request_id, .. }
            | Self::CompleteConnectorHandoff { request_id, .. } => Some(request_id),
            _ => None,
        }
    }

    pub fn validate(&self) -> ContractResult<()> {
        match self {
            Self::ListAccounts { .. } | Self::ListApprovals { .. } => Ok(()),
            Self::CreateManagedAccount {
                chain_namespace,
                label,
                ..
            } => {
                validate_token("chain_namespace", chain_namespace, 64)?;
                validate_optional("label", label.as_deref(), 256)
            }
            Self::LinkVerifiedAccount {
                proof_binding_id,
                chain_namespace,
                address,
                proof_type,
                label,
            } => {
                validate_required("proof_binding_id", proof_binding_id, 256)?;
                validate_token("chain_namespace", chain_namespace, 64)?;
                validate_required("address", address, 256)?;
                validate_token("proof_type", proof_type, 64)?;
                if proof_type.to_ascii_lowercase().contains("managed") {
                    return Err(ContractError::new(
                        "LinkVerifiedAccount rejects managed proof types",
                    ));
                }
                validate_optional("label", label.as_deref(), 256)
            }
            Self::RevokeAccount { account_id } | Self::ExportManagedRecoveryKey { account_id } => {
                validate_required("account_id", account_id, 256)
            }
            Self::RenameAccount { account_id, label } => {
                validate_required("account_id", account_id, 256)?;
                validate_label(label)
            }
            Self::SetDefaultAccount {
                chain_namespace,
                intent,
                account_id,
            } => {
                validate_token("chain_namespace", chain_namespace, 64)?;
                validate_token("default intent", intent, 64)?;
                validate_required("account_id", account_id, 256)
            }
            Self::DefaultAccount {
                chain_namespace,
                intent,
            } => {
                validate_token("chain_namespace", chain_namespace, 64)?;
                validate_token("default intent", intent, 64)
            }
            Self::Challenge {
                domain,
                uri,
                address,
                chain_id,
                resources,
            } => {
                validate_required("domain", domain, 256)?;
                validate_required("uri", uri, 2048)?;
                validate_evm_address("address", address)?;
                if *chain_id == 0 {
                    return Err(ContractError::new("challenge chain_id is required"));
                }
                validate_resources(resources)
            }
            Self::BitcoinChallenge {
                domain,
                uri,
                address,
                network,
                resources,
            } => {
                validate_required("domain", domain, 256)?;
                validate_required("uri", uri, 2048)?;
                validate_required("address", address, 256)?;
                PublicNetwork::new(network.as_str())?;
                validate_resources(resources)
            }
            Self::VerifyProof { message, signature } => {
                validate_signed_message(message)?;
                validate_required("signature", signature, 4096)
            }
            Self::VerifyContractProof {
                message,
                signature,
                evidence,
            } => {
                validate_signed_message(message)?;
                validate_required("signature", signature, 4096)?;
                evidence.validate()
            }
            Self::VerifyBip322Proof {
                message,
                signature,
                signature_type,
                public_key,
            } => {
                validate_signed_message(message)?;
                validate_required("signature", signature, 16 * 1024)?;
                validate_token("signature_type", signature_type, 64)?;
                validate_optional("public_key", public_key.as_deref(), 4096)
            }
            Self::RequestApproval {
                account_id,
                chain_namespace,
                intent,
                resource,
                reason,
                payload,
                expires_at,
            } => {
                validate_required("account_id", account_id, 256)?;
                validate_token("chain_namespace", chain_namespace, 64)?;
                validate_token("approval intent", intent, 128)?;
                validate_required("resource", resource, 2048)?;
                validate_required("reason", reason, 2048)?;
                if payload.is_null() {
                    return Err(ContractError::new("approval payload is required"));
                }
                validate_json_size("approval payload", payload, MAX_APPROVAL_PAYLOAD_BYTES)?;
                if *expires_at == 0 {
                    return Err(ContractError::new("approval expires_at is required"));
                }
                Ok(())
            }
            Self::RejectApproval { request_id, reason }
            | Self::ApproveAndSignManaged { request_id, reason }
            | Self::ApproveConnectorHandoff { request_id, reason } => {
                validate_required("approval request_id", request_id, 256)?;
                validate_required("reason", reason, 2048)
            }
            Self::CompleteConnectorHandoff {
                request_id,
                payload_hash,
                signature,
                signature_type,
                public_key,
                signer,
                transaction_hash,
            } => {
                validate_required("approval request_id", request_id, 256)?;
                validate_hash32("payload_hash", payload_hash)?;
                validate_required("signer", signer, 256)?;
                let has_signature = signature.as_deref().is_some_and(|value| !value.is_empty());
                let has_transaction = transaction_hash
                    .as_deref()
                    .is_some_and(|value| !value.is_empty());
                if has_signature == has_transaction {
                    return Err(ContractError::new(
                        "connector completion requires exactly one signature or transaction_hash",
                    ));
                }
                if has_signature {
                    if transaction_hash.is_some() {
                        return Err(ContractError::new(
                            "signature completion must not carry transaction_hash",
                        ));
                    }
                    validate_optional("signature", signature.as_deref(), 16 * 1024)?;
                    validate_optional("signature_type", signature_type.as_deref(), 64)?;
                    validate_optional("public_key", public_key.as_deref(), 4096)?;
                    if signature_type.is_none() {
                        return Err(ContractError::new(
                            "signature completion requires signature_type and signer",
                        ));
                    }
                } else {
                    validate_optional("transaction_hash", transaction_hash.as_deref(), 512)?;
                    if signature.is_some() || signature_type.is_some() || public_key.is_some() {
                        return Err(ContractError::new(
                            "transaction completion must not carry signature metadata",
                        ));
                    }
                }
                Ok(())
            }
            Self::ImportManagedRecoveryKey {
                recovery_key,
                label,
            } => {
                if !recovery_key.is_object() {
                    return Err(ContractError::new(
                        "recovery_key must be a non-null JSON object",
                    ));
                }
                validate_json_size("recovery_key", recovery_key, MAX_RECOVERY_KEY_BYTES)?;
                validate_optional("label", label.as_deref(), 256)
            }
            Self::ExportManagedRecoverySet {} => Ok(()),
            Self::ImportManagedRecoverySet { recovery_set } => recovery_set.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletProviderRequestV2 {
    pub schema: String,
    pub protocol_version: String,
    pub request_id: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub authority: WalletAuthorityV2,
    pub operation: WalletProviderOperationV2,
    pub request_sha256: String,
    pub session_binding: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub account_binding: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub approval_binding: Option<String>,
    pub lifecycle_id: String,
    pub audit_id: String,
}

impl WalletProviderRequestV2 {
    pub fn new(
        context: &VerifiedWalletInvocationContext,
        request_id: impl Into<String>,
        issued_at: u64,
        expires_at: u64,
        operation: WalletProviderOperationV2,
    ) -> ContractResult<Self> {
        context.validate()?;
        operation.validate()?;
        let request_id = request_id.into();
        validate_request_id(&request_id)?;
        validate_lifetime(issued_at, expires_at, None)?;
        let authority = WalletAuthorityV2::derive(context, &operation);
        let request_sha256 = request_hash(&operation)?;
        let session_binding = derive_session_binding(&authority)?;
        let account_binding = operation
            .account_id()
            .map(|account_id| derive_account_binding(&authority.principal_id, account_id))
            .transpose()?;
        let approval_binding = operation
            .approval_request_id()
            .map(|approval_id| derive_approval_binding(&authority.principal_id, approval_id))
            .transpose()?;
        let lifecycle_id = derive_lifecycle_id(
            &request_id,
            issued_at,
            expires_at,
            &authority,
            &request_sha256,
            &session_binding,
            account_binding.as_deref(),
            approval_binding.as_deref(),
        )?;
        let audit_id = derive_audit_id(&lifecycle_id, &request_sha256)?;
        Ok(Self {
            schema: WALLET_REQUEST_SCHEMA.to_string(),
            protocol_version: WALLET_PROTOCOL_VERSION.to_string(),
            request_id,
            issued_at,
            expires_at,
            authority,
            operation,
            request_sha256,
            session_binding,
            account_binding,
            approval_binding,
            lifecycle_id,
            audit_id,
        })
    }

    pub fn decode_at(bytes: &[u8], now: u64) -> ContractResult<Self> {
        require_version(bytes, WALLET_REQUEST_SCHEMA)?;
        let request: Self = serde_json::from_slice(bytes)?;
        request.validate_at(now)?;
        Ok(request)
    }

    pub fn validate_at(&self, now: u64) -> ContractResult<()> {
        require_exact_version(&self.schema, &self.protocol_version, WALLET_REQUEST_SCHEMA)?;
        validate_request_id(&self.request_id)?;
        validate_lifetime(self.issued_at, self.expires_at, Some(now))?;
        self.authority.validate()?;
        self.operation.validate()?;
        if self.authority.capability != self.operation.capability()
            || self.authority.intent != self.operation.authority_intent()
        {
            return Err(ContractError::new(
                "Wallet request capability or intent was not derived from its operation",
            ));
        }
        let request_sha256 = request_hash(&self.operation)?;
        if self.request_sha256 != request_sha256 {
            return Err(ContractError::new("Wallet request operation hash mismatch"));
        }
        let session_binding = derive_session_binding(&self.authority)?;
        if self.session_binding != session_binding {
            return Err(ContractError::new(
                "Wallet request session binding mismatch",
            ));
        }
        let account_binding = self
            .operation
            .account_id()
            .map(|account_id| derive_account_binding(&self.authority.principal_id, account_id))
            .transpose()?;
        if self.account_binding != account_binding {
            return Err(ContractError::new(
                "Wallet request account binding mismatch",
            ));
        }
        let approval_binding = self
            .operation
            .approval_request_id()
            .map(|approval_id| derive_approval_binding(&self.authority.principal_id, approval_id))
            .transpose()?;
        if self.approval_binding != approval_binding {
            return Err(ContractError::new(
                "Wallet request approval binding mismatch",
            ));
        }
        let lifecycle_id = derive_lifecycle_id(
            &self.request_id,
            self.issued_at,
            self.expires_at,
            &self.authority,
            &self.request_sha256,
            &self.session_binding,
            self.account_binding.as_deref(),
            self.approval_binding.as_deref(),
        )?;
        if self.lifecycle_id != lifecycle_id {
            return Err(ContractError::new(
                "Wallet request lifecycle binding mismatch",
            ));
        }
        let audit_id = derive_audit_id(&self.lifecycle_id, &self.request_sha256)?;
        if self.audit_id != audit_id {
            return Err(ContractError::new("Wallet request audit binding mismatch"));
        }
        Ok(())
    }

    pub fn operation_kind(&self) -> WalletOperationKind {
        self.operation.kind()
    }

    pub fn is_effectful(&self) -> bool {
        self.operation.is_effectful()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum WalletResultV2 {
    Ok { data: Value },
    Error { code: String, message: String },
}

impl WalletResultV2 {
    fn validate(&self) -> ContractResult<()> {
        match self {
            Self::Ok { .. } => Ok(()),
            Self::Error { code, message } => {
                validate_token("Wallet response error code", code, 128)?;
                validate_required("Wallet response error message", message, 4096)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletProviderResponseV2 {
    pub schema: String,
    pub protocol_version: String,
    pub request_id: String,
    pub operation: WalletOperationKind,
    pub audit_id: String,
    pub lifecycle_id: String,
    pub session_binding: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub account_binding: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub approval_binding: Option<String>,
    pub result: WalletResultV2,
}

impl WalletProviderResponseV2 {
    pub fn for_request(request: &WalletProviderRequestV2, result: WalletResultV2) -> Self {
        Self {
            schema: WALLET_RESPONSE_SCHEMA.to_string(),
            protocol_version: WALLET_PROTOCOL_VERSION.to_string(),
            request_id: request.request_id.clone(),
            operation: request.operation.kind(),
            audit_id: request.audit_id.clone(),
            lifecycle_id: request.lifecycle_id.clone(),
            session_binding: request.session_binding.clone(),
            account_binding: request.account_binding.clone(),
            approval_binding: request.approval_binding.clone(),
            result,
        }
    }

    pub fn decode_for_request(
        bytes: &[u8],
        request: &WalletProviderRequestV2,
    ) -> ContractResult<Self> {
        require_version(bytes, WALLET_RESPONSE_SCHEMA)?;
        let response: Self = serde_json::from_slice(bytes)?;
        response.validate_for_request(request)?;
        Ok(response)
    }

    pub fn validate_for_request(&self, request: &WalletProviderRequestV2) -> ContractResult<()> {
        require_exact_version(&self.schema, &self.protocol_version, WALLET_RESPONSE_SCHEMA)?;
        self.result.validate()?;
        if self.request_id != request.request_id
            || self.operation != request.operation.kind()
            || self.audit_id != request.audit_id
            || self.lifecycle_id != request.lifecycle_id
            || self.session_binding != request.session_binding
            || self.account_binding != request.account_binding
            || self.approval_binding != request.approval_binding
        {
            return Err(ContractError::new(
                "Wallet response does not match its authority-bound request",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct VersionProbe {
    schema: Option<String>,
    protocol_version: Option<String>,
}

fn require_version(bytes: &[u8], expected_schema: &str) -> ContractResult<()> {
    let probe: VersionProbe = serde_json::from_slice(bytes)?;
    let schema = probe
        .schema
        .ok_or_else(|| ContractError::new("Wallet contract schema is missing"))?;
    let protocol_version = probe
        .protocol_version
        .ok_or_else(|| ContractError::new("Wallet protocol_version is missing"))?;
    require_exact_version(&schema, &protocol_version, expected_schema)
}

fn require_exact_version(
    schema: &str,
    protocol_version: &str,
    expected_schema: &str,
) -> ContractResult<()> {
    if schema != expected_schema {
        return Err(ContractError::new(format!(
            "unsupported Wallet contract schema {schema}"
        )));
    }
    if protocol_version != WALLET_PROTOCOL_VERSION {
        return Err(ContractError::new(format!(
            "unsupported Wallet protocol_version {protocol_version}"
        )));
    }
    Ok(())
}

fn validate_lifetime(issued_at: u64, expires_at: u64, now: Option<u64>) -> ContractResult<()> {
    if issued_at == 0 || expires_at <= issued_at {
        return Err(ContractError::new("Wallet request lifetime is invalid"));
    }
    if expires_at.saturating_sub(issued_at) > MAX_INVOCATION_TTL_SECS {
        return Err(ContractError::new(
            "Wallet request lifetime exceeds its bound",
        ));
    }
    if let Some(now) = now {
        if issued_at > now.saturating_add(MAX_CLOCK_SKEW_SECS) {
            return Err(ContractError::new(
                "Wallet request was issued in the future",
            ));
        }
        if expires_at <= now {
            return Err(ContractError::new("Wallet request expired"));
        }
    }
    Ok(())
}

fn validate_request_id(value: &str) -> ContractResult<()> {
    let suffix = value
        .strip_prefix(REQUEST_ID_PREFIX)
        .ok_or_else(|| ContractError::new("Wallet request_id has an invalid prefix"))?;
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ContractError::new(
            "Wallet request_id must contain 128 bits of lowercase hexadecimal entropy",
        ));
    }
    Ok(())
}

fn validate_required(label: &str, value: &str, max_len: usize) -> ContractResult<()> {
    if value.is_empty()
        || value.len() > max_len
        || value.chars().any(|character| character.is_control())
    {
        return Err(ContractError::new(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_optional(label: &str, value: Option<&str>, max_len: usize) -> ContractResult<()> {
    if let Some(value) = value {
        validate_required(label, value, max_len)?;
    }
    Ok(())
}

fn validate_signed_message(value: &str) -> ContractResult<()> {
    if value.is_empty() || value.len() > MAX_SIGNED_MESSAGE_BYTES {
        return Err(ContractError::new("message is invalid"));
    }

    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        match character {
            '\n' => {}
            '\r' if characters.next() == Some('\n') => {}
            character if character.is_control() => {
                return Err(ContractError::new("message is invalid"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_token(label: &str, value: &str, max_len: usize) -> ContractResult<()> {
    validate_required(label, value, max_len)?;
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-' | b'/')
    }) {
        return Err(ContractError::new(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_label(value: &str) -> ContractResult<()> {
    validate_required("label", value, 256)
}

fn validate_json_size(label: &str, value: &Value, max_len: usize) -> ContractResult<()> {
    if serde_json::to_vec(value)?.len() > max_len {
        return Err(ContractError::new(format!(
            "{label} exceeds {max_len} serialized bytes"
        )));
    }
    Ok(())
}

fn validate_resources(resources: &[String]) -> ContractResult<()> {
    if resources.len() > 64 {
        return Err(ContractError::new("too many proof resources"));
    }
    for resource in resources {
        validate_required("proof resource", resource, 2048)?;
    }
    Ok(())
}

fn validate_evm_address(label: &str, value: &str) -> ContractResult<()> {
    let hex = value
        .strip_prefix("0x")
        .ok_or_else(|| ContractError::new(format!("{label} is not an EVM address")))?;
    if hex.len() != 40 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ContractError::new(format!("{label} is not an EVM address")));
    }
    Ok(())
}

fn validate_hash32(label: &str, value: &str) -> ContractResult<()> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("sha256:"))
        .ok_or_else(|| ContractError::new(format!("{label} is not a 32-byte hash")))?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ContractError::new(format!("{label} is not a 32-byte hash")));
    }
    Ok(())
}

fn request_hash(operation: &WalletProviderOperationV2) -> ContractResult<String> {
    tagged_hash("request", "elastos.wallet.operation.v2", operation)
}

fn derive_session_binding(authority: &WalletAuthorityV2) -> ContractResult<String> {
    tagged_hash(
        "session",
        "elastos.wallet.session-binding.v2",
        &json!({
            "principal_id": authority.principal_id,
            "session_id": authority.session_id,
            "grant_id": authority.grant_id,
        }),
    )
}

fn derive_account_binding(principal_id: &str, account_id: &str) -> ContractResult<String> {
    validate_required("account_id", account_id, 256)?;
    tagged_hash(
        "account",
        "elastos.wallet.account-binding.v2",
        &json!({ "principal_id": principal_id, "account_id": account_id }),
    )
}

fn derive_approval_binding(principal_id: &str, request_id: &str) -> ContractResult<String> {
    validate_required("approval request_id", request_id, 256)?;
    tagged_hash(
        "approval",
        "elastos.wallet.approval-binding.v2",
        &json!({ "principal_id": principal_id, "request_id": request_id }),
    )
}

#[allow(clippy::too_many_arguments)]
/// Derives an unkeyed integrity and correlation identifier.
///
/// This is not a MAC or signature and does not authenticate the caller. The
/// authenticated Runtime-local provider plane supplies caller authority before
/// a Wallet request is constructed.
fn derive_lifecycle_id(
    request_id: &str,
    issued_at: u64,
    expires_at: u64,
    authority: &WalletAuthorityV2,
    request_sha256: &str,
    session_binding: &str,
    account_binding: Option<&str>,
    approval_binding: Option<&str>,
) -> ContractResult<String> {
    tagged_hash(
        "lifecycle",
        "elastos.wallet.lifecycle.v2",
        &json!({
            "request_id": request_id,
            "issued_at": issued_at,
            "expires_at": expires_at,
            "principal_id": authority.principal_id,
            "session_id": authority.session_id,
            "proof_binding_id": authority.proof_binding_id,
            "grant_id": authority.grant_id,
            "actor": authority.actor,
            "launch_id": authority.launch_id,
            "capability": authority.capability,
            "intent": authority.intent,
            "request_sha256": request_sha256,
            "session_binding": session_binding,
            "account_binding": account_binding,
            "approval_binding": approval_binding,
        }),
    )
}

fn derive_audit_id(lifecycle_id: &str, request_sha256: &str) -> ContractResult<String> {
    tagged_hash(
        "audit",
        "elastos.wallet.audit.v2",
        &json!({
            "lifecycle_id": lifecycle_id,
            "request_sha256": request_sha256,
        }),
    )
}

fn tagged_hash(tag: &str, domain: &str, value: &impl Serialize) -> ContractResult<String> {
    let canonical = canonical_json(serde_json::to_value(value)?);
    let bytes = serde_json::to_vec(&canonical)?;
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0]);
    digest.update(bytes);
    Ok(format!("{tag}:sha256:{}", hex::encode(digest.finalize())))
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_json(value)))
                    .collect(),
            )
        }
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const REQUEST_ID: &str = "wallet-request:00112233445566778899aabbccddeeff";
    const OTHER_REQUEST_ID: &str = "wallet-request:ffeeddccbbaa99887766554433221100";
    const ACCOUNT_ID: &str = "wallet-account:managed:eip155:ela:1";
    const APPROVAL_ID: &str = "wallet-approval:0011223344556677";
    const EVM_ADDRESS: &str = "0x1111111111111111111111111111111111111111";
    const HASH: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn context() -> VerifiedWalletInvocationContext {
        VerifiedWalletInvocationContext::new(
            "person:local:alice",
            "session:alice",
            Some("proof:passkey:alice".to_string()),
            "grant:alice",
            "wallet",
            "launch:00112233445566778899aabbccddeeff",
        )
        .unwrap()
    }

    fn evidence() -> Erc1271ProofEvidenceV1 {
        Erc1271ProofEvidenceV1 {
            schema: ERC1271_EVIDENCE_SCHEMA.to_string(),
            network: PublicNetwork::new("ela-mainnet").unwrap(),
            chain_id: 20,
            contract: EVM_ADDRESS.to_string(),
            message_hash: HASH.to_string(),
            signature_hash: HASH.to_string(),
            valid: true,
            magic_value: ERC1271_MAGIC_VALUE.to_string(),
            checked_at: NOW,
        }
    }

    fn recovery_key() -> Value {
        json!({
            "schema": "elastos.wallet.recovery-key/v1",
            "account_id": ACCOUNT_ID,
            "chain_namespace": "eip155",
            "address": EVM_ADDRESS,
            "secret_type": "secp256k1_private_key_hex",
            "private_key_hex": "1111111111111111111111111111111111111111111111111111111111111111"
        })
    }

    fn recovery_set() -> ManagedRecoverySetV1 {
        ManagedRecoverySetV1::new(vec![ManagedRecoveryKeyEntryV1 {
            account_id: ACCOUNT_ID.to_string(),
            recovery_key: recovery_key(),
            label: Some("Recovered".to_string()),
        }])
        .unwrap()
    }

    fn operations() -> Vec<(WalletOperationKind, WalletProviderOperationV2)> {
        vec![
            (
                WalletOperationKind::ListAccounts,
                WalletProviderOperationV2::ListAccounts {
                    include_revoked: false,
                },
            ),
            (
                WalletOperationKind::CreateManagedAccount,
                WalletProviderOperationV2::CreateManagedAccount {
                    chain_namespace: "eip155".to_string(),
                    label: Some("Managed".to_string()),
                    create_new: true,
                },
            ),
            (
                WalletOperationKind::LinkVerifiedAccount,
                WalletProviderOperationV2::LinkVerifiedAccount {
                    proof_binding_id: "proof:wallet:eip155:20:0x1111".to_string(),
                    chain_namespace: "eip155".to_string(),
                    address: EVM_ADDRESS.to_string(),
                    proof_type: "siwe_eoa".to_string(),
                    label: Some("Connector".to_string()),
                },
            ),
            (
                WalletOperationKind::RevokeAccount,
                WalletProviderOperationV2::RevokeAccount {
                    account_id: ACCOUNT_ID.to_string(),
                },
            ),
            (
                WalletOperationKind::RenameAccount,
                WalletProviderOperationV2::RenameAccount {
                    account_id: ACCOUNT_ID.to_string(),
                    label: "Renamed".to_string(),
                },
            ),
            (
                WalletOperationKind::SetDefaultAccount,
                WalletProviderOperationV2::SetDefaultAccount {
                    chain_namespace: "eip155".to_string(),
                    intent: "sign".to_string(),
                    account_id: ACCOUNT_ID.to_string(),
                },
            ),
            (
                WalletOperationKind::DefaultAccount,
                WalletProviderOperationV2::DefaultAccount {
                    chain_namespace: "eip155".to_string(),
                    intent: "sign".to_string(),
                },
            ),
            (
                WalletOperationKind::Challenge,
                WalletProviderOperationV2::Challenge {
                    domain: "localhost".to_string(),
                    uri: "http://localhost/apps/home/".to_string(),
                    address: EVM_ADDRESS.to_string(),
                    chain_id: 20,
                    resources: vec!["elastos://wallet/link".to_string()],
                },
            ),
            (
                WalletOperationKind::BitcoinChallenge,
                WalletProviderOperationV2::BitcoinChallenge {
                    domain: "localhost".to_string(),
                    uri: "http://localhost/apps/home/".to_string(),
                    address: "bc1qexample".to_string(),
                    network: PublicNetwork::bitcoin(),
                    resources: vec!["elastos://wallet/link".to_string()],
                },
            ),
            (
                WalletOperationKind::VerifyProof,
                WalletProviderOperationV2::VerifyProof {
                    message: "SIWE message".to_string(),
                    signature: "0xsignature".to_string(),
                },
            ),
            (
                WalletOperationKind::VerifyContractProof,
                WalletProviderOperationV2::VerifyContractProof {
                    message: "SIWE message".to_string(),
                    signature: "0xsignature".to_string(),
                    evidence: evidence(),
                },
            ),
            (
                WalletOperationKind::VerifyBip322Proof,
                WalletProviderOperationV2::VerifyBip322Proof {
                    message: "Bitcoin message".to_string(),
                    signature: "signature".to_string(),
                    signature_type: "bip322_simple".to_string(),
                    public_key: Some("02abcdef".to_string()),
                },
            ),
            (
                WalletOperationKind::RequestApproval,
                WalletProviderOperationV2::RequestApproval {
                    account_id: ACCOUNT_ID.to_string(),
                    chain_namespace: "eip155".to_string(),
                    intent: "personal_sign".to_string(),
                    resource: "https://dapp.example".to_string(),
                    reason: "Sign in".to_string(),
                    payload: json!({ "message": "hello" }),
                    expires_at: NOW + 120,
                },
            ),
            (
                WalletOperationKind::ListApprovals,
                WalletProviderOperationV2::ListApprovals {
                    include_resolved: false,
                },
            ),
            (
                WalletOperationKind::RejectApproval,
                WalletProviderOperationV2::RejectApproval {
                    request_id: APPROVAL_ID.to_string(),
                    reason: "Rejected".to_string(),
                },
            ),
            (
                WalletOperationKind::ApproveAndSignManaged,
                WalletProviderOperationV2::ApproveAndSignManaged {
                    request_id: APPROVAL_ID.to_string(),
                    reason: "Approved".to_string(),
                },
            ),
            (
                WalletOperationKind::ApproveConnectorHandoff,
                WalletProviderOperationV2::ApproveConnectorHandoff {
                    request_id: APPROVAL_ID.to_string(),
                    reason: "Approved".to_string(),
                },
            ),
            (
                WalletOperationKind::CompleteConnectorHandoff,
                WalletProviderOperationV2::CompleteConnectorHandoff {
                    request_id: APPROVAL_ID.to_string(),
                    payload_hash: HASH.to_string(),
                    signature: Some("0xsignature".to_string()),
                    signature_type: Some("evm_personal_sign".to_string()),
                    public_key: None,
                    signer: EVM_ADDRESS.to_string(),
                    transaction_hash: None,
                },
            ),
            (
                WalletOperationKind::ExportManagedRecoveryKey,
                WalletProviderOperationV2::ExportManagedRecoveryKey {
                    account_id: ACCOUNT_ID.to_string(),
                },
            ),
            (
                WalletOperationKind::ImportManagedRecoveryKey,
                WalletProviderOperationV2::ImportManagedRecoveryKey {
                    recovery_key: recovery_key(),
                    label: Some("Recovered".to_string()),
                },
            ),
            (
                WalletOperationKind::ExportManagedRecoverySet,
                WalletProviderOperationV2::ExportManagedRecoverySet {},
            ),
            (
                WalletOperationKind::ImportManagedRecoverySet,
                WalletProviderOperationV2::ImportManagedRecoverySet {
                    recovery_set: recovery_set(),
                },
            ),
        ]
    }

    fn request(operation: WalletProviderOperationV2) -> WalletProviderRequestV2 {
        WalletProviderRequestV2::new(&context(), REQUEST_ID, NOW, NOW + 120, operation).unwrap()
    }

    fn signed_message_operations(message: &str) -> [WalletProviderOperationV2; 3] {
        [
            WalletProviderOperationV2::VerifyProof {
                message: message.to_string(),
                signature: "0xsignature".to_string(),
            },
            WalletProviderOperationV2::VerifyContractProof {
                message: message.to_string(),
                signature: "0xsignature".to_string(),
                evidence: evidence(),
            },
            WalletProviderOperationV2::VerifyBip322Proof {
                message: message.to_string(),
                signature: "signature".to_string(),
                signature_type: "bip322_simple".to_string(),
                public_key: Some("02abcdef".to_string()),
            },
        ]
    }

    #[test]
    fn wire_contract_constants_are_exact() {
        assert_eq!(WALLET_BUS_OPERATION, "wallet_contract");
        assert_eq!(WALLET_PROTOCOL_VERSION, "2.1");
        assert_eq!(WALLET_REQUEST_SCHEMA, "elastos.wallet.provider-request/v2");
        assert_eq!(
            WALLET_RESPONSE_SCHEMA,
            "elastos.wallet.provider-response/v2"
        );
        assert_eq!(ERC1271_EVIDENCE_SCHEMA, "elastos.chain.erc1271_proof/v1");
        assert_eq!(
            MANAGED_RECOVERY_SET_SCHEMA,
            "elastos.wallet.managed-recovery-set/v1"
        );
    }

    #[test]
    fn every_production_operation_round_trips_through_the_v2_request_decoder() {
        for (kind, operation) in operations() {
            let request = request(operation);
            assert_eq!(request.operation.kind(), kind);
            let bytes = serde_json::to_vec(&request).unwrap();
            let decoded = WalletProviderRequestV2::decode_at(&bytes, NOW + 1).unwrap();
            assert_eq!(decoded, request);
            assert_eq!(decoded.operation.kind().as_str(), kind.as_str());
        }
    }

    #[test]
    fn signed_messages_accept_text_lf_and_canonical_crlf_within_the_size_bound() {
        for message in [
            "ordinary signed text",
            "first line\nsecond line",
            "first line\r\nsecond line",
            &"x".repeat(MAX_SIGNED_MESSAGE_BYTES),
        ] {
            for operation in signed_message_operations(message) {
                operation.validate().unwrap();
            }
        }
    }

    #[test]
    fn signed_messages_reject_bare_cr_and_all_other_controls() {
        let mut invalid_messages = vec![
            String::new(),
            "before\rafter".to_string(),
            "trailing\r".to_string(),
            "x".repeat(MAX_SIGNED_MESSAGE_BYTES + 1),
        ];
        invalid_messages.extend(
            (0..=0x1f)
                .chain(0x7f..=0x9f)
                .filter_map(char::from_u32)
                .filter(|character| !matches!(character, '\n' | '\r'))
                .map(|character| format!("before{character}after")),
        );

        for message in invalid_messages {
            for operation in signed_message_operations(&message) {
                assert!(operation.validate().is_err(), "accepted {message:?}");
            }
        }
    }

    #[test]
    fn optional_account_labels_may_be_absent_on_the_production_wire() {
        let operations = [
            WalletProviderOperationV2::CreateManagedAccount {
                chain_namespace: "eip155".to_string(),
                label: None,
                create_new: true,
            },
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: "proof:wallet:eip155:20:0x1111".to_string(),
                chain_namespace: "eip155".to_string(),
                address: EVM_ADDRESS.to_string(),
                proof_type: "siwe_eoa".to_string(),
                label: None,
            },
            WalletProviderOperationV2::ImportManagedRecoveryKey {
                recovery_key: recovery_key(),
                label: None,
            },
        ];

        for operation in operations {
            let request = request(operation);
            let value = serde_json::to_value(&request).unwrap();
            assert!(value["operation"]["params"].get("label").is_none());
            let decoded =
                WalletProviderRequestV2::decode_at(&serde_json::to_vec(&value).unwrap(), NOW + 1)
                    .unwrap();
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn structured_recovery_keys_are_bounded_without_parsing_wallet_semantics() {
        let request = request(WalletProviderOperationV2::ImportManagedRecoveryKey {
            recovery_key: recovery_key(),
            label: None,
        });
        let decoded =
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&request).unwrap(), NOW + 1)
                .unwrap();
        assert_eq!(decoded, request);

        for recovery_key in [Value::Null, json!("opaque-secret"), json!(["secret"])] {
            let error = WalletProviderRequestV2::new(
                &context(),
                REQUEST_ID,
                NOW,
                NOW + 120,
                WalletProviderOperationV2::ImportManagedRecoveryKey {
                    recovery_key,
                    label: None,
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("non-null JSON object"));
        }

        let error = WalletProviderRequestV2::new(
            &context(),
            REQUEST_ID,
            NOW,
            NOW + 120,
            WalletProviderOperationV2::ImportManagedRecoveryKey {
                recovery_key: json!({ "opaque": "x".repeat(MAX_RECOVERY_KEY_BYTES) }),
                label: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 65536 serialized bytes"));
    }

    #[test]
    fn managed_recovery_sets_are_closed_bounded_and_unambiguous() {
        let recovery_set = recovery_set();
        recovery_set.validate().unwrap();
        request(WalletProviderOperationV2::ImportManagedRecoverySet {
            recovery_set: recovery_set.clone(),
        });

        let duplicate = ManagedRecoverySetV1::new(vec![
            recovery_set.keys[0].clone(),
            recovery_set.keys[0].clone(),
        ])
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate account"));

        let too_many = (0..=MAX_MANAGED_RECOVERY_SET_KEYS)
            .map(|index| ManagedRecoveryKeyEntryV1 {
                account_id: format!("wallet:eip155:20:0x{index:040x}"),
                recovery_key: json!({"opaque": index}),
                label: None,
            })
            .collect();
        assert!(ManagedRecoverySetV1::new(too_many)
            .unwrap_err()
            .to_string()
            .contains("exceeds 64 keys"));

        let oversized_key = ManagedRecoverySetV1::new(vec![ManagedRecoveryKeyEntryV1 {
            account_id: ACCOUNT_ID.to_string(),
            recovery_key: json!({"opaque": "x".repeat(MAX_RECOVERY_KEY_BYTES)}),
            label: None,
        }])
        .unwrap_err();
        assert!(oversized_key
            .to_string()
            .contains("exceeds 65536 serialized bytes"));

        let aggregate = (0..5)
            .map(|index| ManagedRecoveryKeyEntryV1 {
                account_id: format!("wallet:eip155:20:0x{index:040x}"),
                recovery_key: json!({"opaque": "x".repeat(60 * 1024)}),
                label: None,
            })
            .collect();
        assert!(ManagedRecoverySetV1::new(aggregate)
            .unwrap_err()
            .to_string()
            .contains("exceeds 262144 bytes"));

        for recovery_key in [Value::Null, json!("opaque-secret"), json!(["secret"])] {
            assert!(ManagedRecoverySetV1::new(vec![ManagedRecoveryKeyEntryV1 {
                account_id: ACCOUNT_ID.to_string(),
                recovery_key,
                label: None,
            }])
            .is_err());
        }

        let request = request(WalletProviderOperationV2::ImportManagedRecoverySet { recovery_set });
        let base = serde_json::to_value(&request).unwrap();
        for path in ["set", "entry"] {
            let mut candidate = base.clone();
            match path {
                "set" => candidate["operation"]["params"]["recovery_set"]["unknown"] = json!(true),
                "entry" => {
                    candidate["operation"]["params"]["recovery_set"]["keys"][0]["unknown"] =
                        json!(true)
                }
                _ => unreachable!(),
            }
            assert!(WalletProviderRequestV2::decode_at(
                &serde_json::to_vec(&candidate).unwrap(),
                NOW + 1,
            )
            .unwrap_err()
            .to_string()
            .contains("unknown field"));
        }
        for forbidden in [
            "principal_id",
            "actor",
            "session_id",
            "proof",
            "grant",
            "launch_id",
            "lifecycle_id",
        ] {
            let mut candidate = base.clone();
            candidate["operation"]["params"][forbidden] = json!("caller-controlled");
            assert!(WalletProviderRequestV2::decode_at(
                &serde_json::to_vec(&candidate).unwrap(),
                NOW + 1,
            )
            .unwrap_err()
            .to_string()
            .contains("unknown field"));
        }
    }

    #[test]
    fn connector_signature_and_transaction_completions_preserve_signer() {
        let completions = [
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id: APPROVAL_ID.to_string(),
                payload_hash: HASH.to_string(),
                signature: Some("0xsignature".to_string()),
                signature_type: Some("evm_personal_sign".to_string()),
                public_key: None,
                signer: EVM_ADDRESS.to_string(),
                transaction_hash: None,
            },
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id: APPROVAL_ID.to_string(),
                payload_hash: HASH.to_string(),
                signature: None,
                signature_type: None,
                public_key: None,
                signer: EVM_ADDRESS.to_string(),
                transaction_hash: Some(HASH.to_string()),
            },
        ];

        for completion in completions {
            let request = request(completion);
            let decoded =
                WalletProviderRequestV2::decode_at(&serde_json::to_vec(&request).unwrap(), NOW + 1)
                    .unwrap();
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn connector_completion_rejects_missing_signer_and_mixed_result_fields() {
        let transaction_completion =
            |signature: Option<&str>,
             signature_type: Option<&str>,
             public_key: Option<&str>,
             signer: &str| WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id: APPROVAL_ID.to_string(),
                payload_hash: HASH.to_string(),
                signature: signature.map(str::to_string),
                signature_type: signature_type.map(str::to_string),
                public_key: public_key.map(str::to_string),
                signer: signer.to_string(),
                transaction_hash: Some(HASH.to_string()),
            };
        let valid_transaction = transaction_completion(None, None, None, EVM_ADDRESS);
        let request = request(valid_transaction.clone());
        let mut missing_signer = serde_json::to_value(request).unwrap();
        missing_signer["operation"]["params"]
            .as_object_mut()
            .unwrap()
            .remove("signer");
        assert!(WalletProviderRequestV2::decode_at(
            &serde_json::to_vec(&missing_signer).unwrap(),
            NOW + 1,
        )
        .unwrap_err()
        .to_string()
        .contains("missing field `signer`"));

        let invalid_completions = [
            transaction_completion(
                Some("0xsignature"),
                Some("evm_personal_sign"),
                None,
                EVM_ADDRESS,
            ),
            transaction_completion(None, Some("evm_personal_sign"), None, EVM_ADDRESS),
            transaction_completion(None, None, Some("02abcdef"), EVM_ADDRESS),
            transaction_completion(None, None, None, ""),
        ];

        for completion in invalid_completions {
            assert!(WalletProviderRequestV2::new(
                &context(),
                REQUEST_ID,
                NOW,
                NOW + 120,
                completion,
            )
            .is_err());
        }

        let missing_signature_type = WalletProviderOperationV2::CompleteConnectorHandoff {
            request_id: APPROVAL_ID.to_string(),
            payload_hash: HASH.to_string(),
            signature: Some("0xsignature".to_string()),
            signature_type: None,
            public_key: None,
            signer: EVM_ADDRESS.to_string(),
            transaction_hash: None,
        };
        assert!(WalletProviderRequestV2::new(
            &context(),
            REQUEST_ID,
            NOW,
            NOW + 120,
            missing_signature_type,
        )
        .unwrap_err()
        .to_string()
        .contains("requires signature_type and signer"));

        let signature_with_transaction_field =
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id: APPROVAL_ID.to_string(),
                payload_hash: HASH.to_string(),
                signature: Some("0xsignature".to_string()),
                signature_type: Some("evm_personal_sign".to_string()),
                public_key: None,
                signer: EVM_ADDRESS.to_string(),
                transaction_hash: Some(String::new()),
            };
        assert!(WalletProviderRequestV2::new(
            &context(),
            REQUEST_ID,
            NOW,
            NOW + 120,
            signature_with_transaction_field,
        )
        .unwrap_err()
        .to_string()
        .contains("must not carry transaction_hash"));
    }

    #[test]
    fn approval_payloads_enforce_the_existing_32_kib_limit() {
        let error = WalletProviderRequestV2::new(
            &context(),
            REQUEST_ID,
            NOW,
            NOW + 120,
            WalletProviderOperationV2::RequestApproval {
                account_id: ACCOUNT_ID.to_string(),
                chain_namespace: "eip155".to_string(),
                intent: "personal_sign".to_string(),
                resource: "https://dapp.example".to_string(),
                reason: "Sign in".to_string(),
                payload: json!({ "message": "x".repeat(MAX_APPROVAL_PAYLOAD_BYTES) }),
                expires_at: NOW + 120,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 32768 serialized bytes"));
    }

    #[test]
    fn request_and_nested_operation_reject_unknown_fields() {
        let request = request(WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        });
        let mut value = serde_json::to_value(&request).unwrap();
        value["unknown"] = json!(true);
        assert!(
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&value).unwrap(), NOW + 1)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );

        let mut value = serde_json::to_value(&request).unwrap();
        value["operation"]["params"]["unknown"] = json!(true);
        assert!(
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&value).unwrap(), NOW + 1)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn nullable_authority_and_response_bindings_are_required_fields() {
        let request = request(WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        });
        for field in ["proof_binding_id", "account_binding", "approval_binding"] {
            let mut candidate = serde_json::to_value(&request).unwrap();
            if field == "proof_binding_id" {
                candidate["authority"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
            } else {
                candidate.as_object_mut().unwrap().remove(field);
            }
            assert!(WalletProviderRequestV2::decode_at(
                &serde_json::to_vec(&candidate).unwrap(),
                NOW + 1,
            )
            .is_err());
        }

        let response =
            WalletProviderResponseV2::for_request(&request, WalletResultV2::Ok { data: json!({}) });
        for field in ["account_binding", "approval_binding"] {
            let mut candidate = serde_json::to_value(&response).unwrap();
            candidate.as_object_mut().unwrap().remove(field);
            assert!(WalletProviderResponseV2::decode_for_request(
                &serde_json::to_vec(&candidate).unwrap(),
                &request,
            )
            .is_err());
        }
    }

    #[test]
    fn missing_stale_and_mixed_versions_fail_closed() {
        let request = request(WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        });
        let value = serde_json::to_value(request).unwrap();
        for (schema, version) in [
            (None, Some("2.1")),
            (Some(WALLET_REQUEST_SCHEMA), None),
            (Some("elastos.wallet.provider-request/v1"), Some("2.1")),
            (Some(WALLET_REQUEST_SCHEMA), Some("1.0")),
            (Some(WALLET_REQUEST_SCHEMA), Some("2.0")),
            (Some(WALLET_REQUEST_SCHEMA), Some("2.2")),
        ] {
            let mut candidate = value.clone();
            match schema {
                Some(schema) => candidate["schema"] = json!(schema),
                None => {
                    candidate.as_object_mut().unwrap().remove("schema");
                }
            }
            match version {
                Some(version) => candidate["protocol_version"] = json!(version),
                None => {
                    candidate
                        .as_object_mut()
                        .unwrap()
                        .remove("protocol_version");
                }
            }
            assert!(WalletProviderRequestV2::decode_at(
                &serde_json::to_vec(&candidate).unwrap(),
                NOW + 1
            )
            .is_err());
        }
    }

    #[test]
    fn authority_and_request_bindings_are_derived_and_stable() {
        let operation = WalletProviderOperationV2::RequestApproval {
            account_id: ACCOUNT_ID.to_string(),
            chain_namespace: "eip155".to_string(),
            intent: "personal_sign".to_string(),
            resource: "https://dapp.example".to_string(),
            reason: "Sign".to_string(),
            payload: json!({ "b": 2, "a": 1 }),
            expires_at: NOW + 120,
        };
        let first = request(operation.clone());
        let second = request(operation);
        assert_eq!(first, second);
        assert_eq!(first.authority.capability, "wallet:approval:request");
        assert_eq!(first.authority.intent, "wallet.approval.request");
        assert!(first
            .account_binding
            .as_deref()
            .unwrap()
            .starts_with("account:sha256:"));
        assert!(first.approval_binding.is_none());
        assert!(first.request_sha256.starts_with("request:sha256:"));
        assert!(first.session_binding.starts_with("session:sha256:"));
        assert!(first.lifecycle_id.starts_with("lifecycle:sha256:"));
        assert!(first.audit_id.starts_with("audit:sha256:"));
        assert_eq!(
            first.request_sha256,
            "request:sha256:bf1264dfbf77187e10a6fa9f75c0dbc0c680866be5767b3a84670434b6a259d1",
        );
        assert_eq!(
            first.session_binding,
            "session:sha256:0f3939296e556150b609c699e3ef0a94c46c25104e2977efb3759a6290c7230e",
        );
        assert_eq!(
            first.account_binding.as_deref(),
            Some("account:sha256:0c1b9e586e8d9199fec5314c3453c6bcf08d03581bd199671db66c208fbe0234"),
        );
        assert_eq!(
            first.lifecycle_id,
            "lifecycle:sha256:befb52ab0c82746187076508169470c098297582ba06ef14c0784c82fc029474",
        );
        assert_eq!(
            first.audit_id,
            "audit:sha256:2f723b5d50364ded49c9032b857a3de235c8be02d986d6b261c292a270ca34d2",
        );

        let other = WalletProviderRequestV2::new(
            &context(),
            OTHER_REQUEST_ID,
            NOW,
            NOW + 120,
            first.operation.clone(),
        )
        .unwrap();
        assert_ne!(first.lifecycle_id, other.lifecycle_id);
        assert_ne!(first.audit_id, other.audit_id);
    }

    #[test]
    fn request_tampering_breaks_operation_and_authority_bindings() {
        let request = request(WalletProviderOperationV2::RenameAccount {
            account_id: ACCOUNT_ID.to_string(),
            label: "Original".to_string(),
        });
        let mut value = serde_json::to_value(&request).unwrap();
        value["operation"]["params"]["label"] = json!("Substituted");
        let error =
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&value).unwrap(), NOW + 1)
                .unwrap_err();
        assert!(error.to_string().contains("operation hash mismatch"));

        let mut value = serde_json::to_value(&request).unwrap();
        value["authority"]["actor"] = json!("wallet-metamask");
        let error =
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&value).unwrap(), NOW + 1)
                .unwrap_err();
        assert!(error.to_string().contains("lifecycle binding mismatch"));

        let mut value = serde_json::to_value(&request).unwrap();
        value["authority"]["capability"] = json!("wallet:recovery:export-managed");
        let error =
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&value).unwrap(), NOW + 1)
                .unwrap_err();
        assert!(error.to_string().contains("not derived"));
    }

    #[test]
    fn account_and_approval_selectors_have_distinct_principal_bound_hashes() {
        let account = request(WalletProviderOperationV2::RevokeAccount {
            account_id: ACCOUNT_ID.to_string(),
        });
        assert!(account.account_binding.is_some());
        assert!(account.approval_binding.is_none());

        let approval = request(WalletProviderOperationV2::ApproveConnectorHandoff {
            request_id: APPROVAL_ID.to_string(),
            reason: "Approved".to_string(),
        });
        assert!(approval.account_binding.is_none());
        assert!(approval.approval_binding.is_some());
        assert_ne!(account.lifecycle_id, approval.lifecycle_id);

        let mut substituted = serde_json::to_value(&approval).unwrap();
        substituted["operation"]["params"]["request_id"] = json!("wallet-approval:attacker");
        assert!(WalletProviderRequestV2::decode_at(
            &serde_json::to_vec(&substituted).unwrap(),
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("operation hash mismatch"));
    }

    #[test]
    fn request_lifetimes_are_bounded_and_checked_at_decode() {
        let operation = WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        };
        assert!(WalletProviderRequestV2::new(
            &context(),
            REQUEST_ID,
            NOW,
            NOW + MAX_INVOCATION_TTL_SECS + 1,
            operation.clone(),
        )
        .unwrap_err()
        .to_string()
        .contains("exceeds"));

        let request =
            WalletProviderRequestV2::new(&context(), REQUEST_ID, NOW, NOW + 1, operation.clone())
                .unwrap();
        assert!(WalletProviderRequestV2::decode_at(
            &serde_json::to_vec(&request).unwrap(),
            NOW + 1
        )
        .unwrap_err()
        .to_string()
        .contains("expired"));

        let request = WalletProviderRequestV2::new(
            &context(),
            REQUEST_ID,
            NOW + MAX_CLOCK_SKEW_SECS + 1,
            NOW + MAX_CLOCK_SKEW_SECS + 2,
            operation,
        )
        .unwrap();
        assert!(
            WalletProviderRequestV2::decode_at(&serde_json::to_vec(&request).unwrap(), NOW)
                .unwrap_err()
                .to_string()
                .contains("future")
        );
    }

    #[test]
    fn response_decoder_rejects_version_and_binding_substitution() {
        let request = request(WalletProviderOperationV2::ApproveAndSignManaged {
            request_id: APPROVAL_ID.to_string(),
            reason: "Approved".to_string(),
        });
        let response = WalletProviderResponseV2::for_request(
            &request,
            WalletResultV2::Ok {
                data: json!({ "signature": "0xsignature" }),
            },
        );
        WalletProviderResponseV2::decode_for_request(
            &serde_json::to_vec(&response).unwrap(),
            &request,
        )
        .unwrap();

        let mut unknown_response = serde_json::to_value(&response).unwrap();
        unknown_response["unknown"] = json!(true);
        assert!(WalletProviderResponseV2::decode_for_request(
            &serde_json::to_vec(&unknown_response).unwrap(),
            &request,
        )
        .unwrap_err()
        .to_string()
        .contains("unknown field"));

        let mut unknown_result = serde_json::to_value(&response).unwrap();
        unknown_result["result"]["unknown"] = json!(true);
        assert!(WalletProviderResponseV2::decode_for_request(
            &serde_json::to_vec(&unknown_result).unwrap(),
            &request,
        )
        .unwrap_err()
        .to_string()
        .contains("unknown field"));

        for (field, value) in [
            ("request_id", json!(OTHER_REQUEST_ID)),
            ("audit_id", json!("audit:sha256:attacker")),
            ("lifecycle_id", json!("lifecycle:sha256:attacker")),
            ("session_binding", json!("session:sha256:attacker")),
            ("approval_binding", json!(null)),
        ] {
            let mut candidate = serde_json::to_value(&response).unwrap();
            candidate[field] = value;
            assert!(WalletProviderResponseV2::decode_for_request(
                &serde_json::to_vec(&candidate).unwrap(),
                &request,
            )
            .unwrap_err()
            .to_string()
            .contains("does not match"));
        }

        for (schema, version) in [
            (None, Some("2.1")),
            (Some(WALLET_RESPONSE_SCHEMA), None),
            (Some("elastos.wallet.provider-response/v1"), Some("2.1")),
            (Some(WALLET_RESPONSE_SCHEMA), Some("1.0")),
            (Some(WALLET_RESPONSE_SCHEMA), Some("2.0")),
            (Some(WALLET_RESPONSE_SCHEMA), Some("2.2")),
        ] {
            let mut candidate = serde_json::to_value(&response).unwrap();
            match schema {
                Some(schema) => candidate["schema"] = json!(schema),
                None => {
                    candidate.as_object_mut().unwrap().remove("schema");
                }
            }
            match version {
                Some(version) => candidate["protocol_version"] = json!(version),
                None => {
                    candidate
                        .as_object_mut()
                        .unwrap()
                        .remove("protocol_version");
                }
            }
            assert!(WalletProviderResponseV2::decode_for_request(
                &serde_json::to_vec(&candidate).unwrap(),
                &request,
            )
            .is_err());
        }
    }

    #[test]
    fn only_repeatable_queries_are_classified_as_reads() {
        for (kind, operation) in operations() {
            let expected = matches!(
                kind,
                WalletOperationKind::ListAccounts
                    | WalletOperationKind::DefaultAccount
                    | WalletOperationKind::ListApprovals
            );
            assert_eq!(operation.class() == WalletOperationClass::Read, expected);
            assert_eq!(operation.is_effectful(), !expected);
        }
    }

    #[test]
    fn account_and_approval_operations_preserve_both_authority_classes() {
        let kinds: Vec<_> = operations().into_iter().map(|(kind, _)| kind).collect();
        for required in [
            WalletOperationKind::ListAccounts,
            WalletOperationKind::SetDefaultAccount,
            WalletOperationKind::DefaultAccount,
            WalletOperationKind::ApproveAndSignManaged,
            WalletOperationKind::ApproveConnectorHandoff,
            WalletOperationKind::CompleteConnectorHandoff,
            WalletOperationKind::ExportManagedRecoveryKey,
            WalletOperationKind::ImportManagedRecoveryKey,
            WalletOperationKind::ExportManagedRecoverySet,
            WalletOperationKind::ImportManagedRecoverySet,
        ] {
            assert!(kinds.contains(&required));
        }
    }

    #[test]
    fn erc1271_evidence_is_typed_and_fails_closed() {
        let operation = WalletProviderOperationV2::VerifyContractProof {
            message: "SIWE message".to_string(),
            signature: "0xsignature".to_string(),
            evidence: evidence(),
        };
        request(operation.clone());

        let mut invalid = operation;
        let WalletProviderOperationV2::VerifyContractProof { evidence, .. } = &mut invalid else {
            unreachable!();
        };
        evidence.valid = false;
        assert!(
            WalletProviderRequestV2::new(&context(), REQUEST_ID, NOW, NOW + 120, invalid,)
                .unwrap_err()
                .to_string()
                .contains("not valid")
        );
    }

    #[test]
    fn bitcoin_challenge_defaults_to_the_compatible_bitcoin_network() {
        let request = request(WalletProviderOperationV2::BitcoinChallenge {
            domain: "localhost".to_string(),
            uri: "http://localhost/apps/home/".to_string(),
            address: "bc1qexample".to_string(),
            network: PublicNetwork::bitcoin(),
            resources: vec![],
        });
        let mut value = serde_json::to_value(request).unwrap();
        value["operation"]["params"]
            .as_object_mut()
            .unwrap()
            .remove("network");
        // Recompute bindings by decoding only the operation shape. A missing
        // network is accepted as the compatible `bitcoin` default.
        let operation: WalletProviderOperationV2 =
            serde_json::from_value(value["operation"].clone()).unwrap();
        let WalletProviderOperationV2::BitcoinChallenge { network, .. } = operation else {
            unreachable!();
        };
        assert_eq!(network.as_str(), DEFAULT_BITCOIN_NETWORK);
    }

    #[test]
    fn retired_generic_operations_are_not_part_of_the_production_decoder() {
        for retired in [
            "approve_approval",
            "complete_approval",
            "sign_approved",
            "record_transaction_hash",
        ] {
            let value = json!({ "kind": retired, "params": {} });
            assert!(serde_json::from_value::<WalletProviderOperationV2>(value).is_err());
        }
    }
}
