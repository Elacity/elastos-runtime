//! Buy/open/play coordination on the existing Runtime crate.
//!
//! Wallet must sign the exact rights request. Chain supplies a durable
//! transaction identity from the existing Runtime transaction coordinator.
//! Decrypt returns an opaque viewer handle. Bearer `play_url` values and Home
//! launch tokens are not protected-content authority. Live `decrypt` is not
//! replaced.

use std::fmt;

use elastos_protected_content_contracts::{
    CanonicalContract, ContractError, CustodyEnvelopeV1, Digest32, EncryptedContentIdentityV1,
    KeyEnvelopeIdentityV1, ProtectedContentBindingV1, RecipientKeyIdentityV1,
    RecipientPublicKeyBytesV1, RightsActionV1, RightsPolicyIdentityV1, RightsRequestV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, SignedNodeContributionV1,
    SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
    WalletAddress,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, DecryptProviderRequestV1, DecryptProviderResponseStatusV1,
    DecryptProviderResponseV1, ValidatedDecryptProviderRequestV1, ViewerMediaPartSelectorV1,
    MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
};
use elastos_wallet_contract::{
    PublicNetwork, ValidatedChainOutcomeBindingV1, ValidatedChainOutcomeV1,
    WalletProviderOperationV2, WalletProviderRequestV2, WalletProviderResponseV2,
    VALIDATED_CHAIN_OUTCOME_SCHEMA,
};
use serde_json::Value;
use thiserror::Error;

use crate::coordinator::validate_wallet_rights_signature;
use crate::{PersistedRuntimeMint, RuntimeProviderCallError, RuntimeReleaseCoordinatorError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RuntimeOpenError {
    #[error("runtime buy/open wallet authority is invalid")]
    WalletAuthority,
    #[error("runtime buy/open mint selection is invalid")]
    MintSelection,
    #[error("runtime buy/open chain evidence is invalid")]
    ChainEvidence,
    #[error("runtime open decrypt result is invalid")]
    DecryptResult,
    #[error("runtime open rejected a bearer playback URL")]
    BearerPlaybackUrl,
}

impl From<RuntimeReleaseCoordinatorError> for RuntimeOpenError {
    fn from(error: RuntimeReleaseCoordinatorError) -> Self {
        match error {
            RuntimeReleaseCoordinatorError::WalletAuthority => Self::WalletAuthority,
            _ => Self::DecryptResult,
        }
    }
}

impl From<ContractError> for RuntimeOpenError {
    fn from(_: ContractError) -> Self {
        Self::DecryptResult
    }
}

#[async_trait::async_trait]
pub trait RuntimeDecryptProvider: Send + Sync {
    async fn prepare_recipient(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError>;

    async fn open_viewer_session(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError>;

    async fn read_viewer_media_part(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError>;

    async fn cancel_prepared_recipient(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError>;

    async fn close_viewer_session(
        &self,
        request: &DecryptProviderRequestV1,
    ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError>;
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeOpenViewerSessionInput<'a> {
    pub buy: &'a RuntimeBuyReceipt,
    pub prepared_recipient: &'a RuntimePreparedRecipient,
    pub signed_runtime_release_operation: &'a SignedRuntimeReleaseOperationV1,
    pub expected_terminal_issuer: TerminalReceiptIssuerKey,
    pub custody_envelope: &'a CustodyEnvelopeV1,
    pub media_identity: &'a CencFmp4MediaIdentityV1,
    pub protected_init_segment: &'a [u8],
    pub signed_node_contributions: &'a [SignedNodeContributionV1],
    pub signed_terminal_receipt: &'a SignedTerminalReceiptV1,
    pub now_unix_seconds: u64,
}

const MAX_RUNTIME_PURCHASE_TEXT_BYTES: usize = 256;
const MAX_RUNTIME_PURCHASE_NETWORK_BYTES: usize = 128;
const MAX_RUNTIME_PURCHASE_CHAIN_NAMESPACE_BYTES: usize = 64;
const MAX_RUNTIME_PURCHASE_CALLDATA_BYTES: usize = 32 * 1024;
const MAX_RUNTIME_PURCHASE_VALUE_HEX_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProtectedContentPurchaseIntent {
    mint_id: Digest32,
    encrypted_content: EncryptedContentIdentityV1,
    key_envelope: KeyEnvelopeIdentityV1,
    rights_policy: RightsPolicyIdentityV1,
    action: RightsActionV1,
    chain_namespace: String,
    network: String,
    to: String,
    value: String,
    data: String,
}

impl RuntimeProtectedContentPurchaseIntent {
    #[allow(
        clippy::too_many_arguments,
        reason = "these are the exact independently validated purchase bindings"
    )]
    pub fn new(
        mint_id: Digest32,
        encrypted_content: EncryptedContentIdentityV1,
        key_envelope: KeyEnvelopeIdentityV1,
        rights_policy: RightsPolicyIdentityV1,
        action: RightsActionV1,
        chain_namespace: impl Into<String>,
        network: impl Into<String>,
        to: impl Into<String>,
        value: impl Into<String>,
        data: impl Into<String>,
    ) -> Result<Self, RuntimeOpenError> {
        let value = Self {
            mint_id,
            encrypted_content,
            key_envelope,
            rights_policy,
            action,
            chain_namespace: chain_namespace.into(),
            network: network.into(),
            to: to.into(),
            value: value.into(),
            data: data.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), RuntimeOpenError> {
        validate_required_text(
            &self.chain_namespace,
            MAX_RUNTIME_PURCHASE_CHAIN_NAMESPACE_BYTES,
            RuntimeOpenError::ChainEvidence,
        )?;
        validate_required_text(
            &self.network,
            MAX_RUNTIME_PURCHASE_NETWORK_BYTES,
            RuntimeOpenError::ChainEvidence,
        )?;
        PublicNetwork::new(self.network.as_str()).map_err(|_| RuntimeOpenError::ChainEvidence)?;
        parse_wallet_address(&self.to)?;
        validate_hex_quantity(&self.value, MAX_RUNTIME_PURCHASE_VALUE_HEX_BYTES)?;
        decode_prefixed_hex_bytes(&self.data, MAX_RUNTIME_PURCHASE_CALLDATA_BYTES)?;
        Ok(())
    }

    pub const fn mint_id(&self) -> Digest32 {
        self.mint_id
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub fn key_envelope(&self) -> &KeyEnvelopeIdentityV1 {
        &self.key_envelope
    }

    pub fn rights_policy(&self) -> &RightsPolicyIdentityV1 {
        &self.rights_policy
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub fn chain_namespace(&self) -> &str {
        &self.chain_namespace
    }

    pub fn network(&self) -> &str {
        &self.network
    }

    pub fn to(&self) -> &str {
        &self.to
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn data(&self) -> &str {
        &self.data
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePurchaseEffectAuthority {
    principal_id: String,
    account_id: String,
    address: String,
    approval_request_id: String,
}

impl RuntimePurchaseEffectAuthority {
    pub fn new(
        principal_id: impl Into<String>,
        account_id: impl Into<String>,
        address: impl Into<String>,
        approval_request_id: impl Into<String>,
    ) -> Result<Self, RuntimeOpenError> {
        let value = Self {
            principal_id: principal_id.into(),
            account_id: account_id.into(),
            address: address.into(),
            approval_request_id: approval_request_id.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), RuntimeOpenError> {
        validate_required_text(
            &self.principal_id,
            MAX_RUNTIME_PURCHASE_TEXT_BYTES,
            RuntimeOpenError::WalletAuthority,
        )?;
        validate_required_text(
            &self.account_id,
            MAX_RUNTIME_PURCHASE_TEXT_BYTES,
            RuntimeOpenError::ChainEvidence,
        )?;
        validate_required_text(
            &self.approval_request_id,
            MAX_RUNTIME_PURCHASE_TEXT_BYTES,
            RuntimeOpenError::ChainEvidence,
        )?;
        parse_wallet_address(&self.address)?;
        Ok(())
    }

    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn approval_request_id(&self) -> &str {
        &self.approval_request_id
    }
}

#[derive(Clone, PartialEq)]
pub struct RuntimeVerifiedPurchaseEffect {
    intent: RuntimeProtectedContentPurchaseIntent,
    authority: RuntimePurchaseEffectAuthority,
    wallet_address: WalletAddress,
    chain_outcome: ValidatedChainOutcomeV1,
    chain_transaction: Digest32,
}

impl fmt::Debug for RuntimeVerifiedPurchaseEffect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeVerifiedPurchaseEffect")
            .field("intent", &self.intent)
            .field("principal_id", &self.authority.principal_id)
            .field("account_id", &self.authority.account_id)
            .field("address", &self.authority.address)
            .field("approval_request_id", &self.authority.approval_request_id)
            .field("wallet_binding", &self.chain_outcome.binding)
            .field("transaction_hash", &self.chain_outcome.transaction_hash)
            .field("confirmed_at", &self.chain_outcome.confirmed_at)
            .finish()
    }
}

impl RuntimeVerifiedPurchaseEffect {
    pub fn new(
        intent: RuntimeProtectedContentPurchaseIntent,
        authority: RuntimePurchaseEffectAuthority,
        wallet_binding: ValidatedChainOutcomeBindingV1,
        transaction_hash: impl Into<String>,
        chain_observation: Value,
        confirmed_at: u64,
    ) -> Result<Self, RuntimeOpenError> {
        intent.validate()?;
        authority.validate()?;
        if json_contains_bearer_playback(&chain_observation) {
            return Err(RuntimeOpenError::BearerPlaybackUrl);
        }
        let wallet_address = parse_wallet_address(authority.address())?;
        if let ValidatedChainOutcomeBindingV1::ExternalConnector {
            originating_address,
            ..
        } = &wallet_binding
        {
            let originating = parse_wallet_address(originating_address)?;
            if originating != wallet_address {
                return Err(RuntimeOpenError::ChainEvidence);
            }
        }
        let chain_outcome = ValidatedChainOutcomeV1 {
            schema: VALIDATED_CHAIN_OUTCOME_SCHEMA.to_string(),
            approval_request_id: authority.approval_request_id().to_string(),
            account_id: authority.account_id().to_string(),
            chain_namespace: intent.chain_namespace().to_string(),
            network: PublicNetwork::new(intent.network())
                .map_err(|_| RuntimeOpenError::ChainEvidence)?,
            binding: wallet_binding,
            transaction_hash: transaction_hash.into(),
            chain_observation,
            confirmed_at,
        };
        let chain_transaction = chain_transaction_digest(&chain_outcome)?;
        Ok(Self {
            intent,
            authority,
            wallet_address,
            chain_outcome,
            chain_transaction,
        })
    }

    pub fn intent(&self) -> &RuntimeProtectedContentPurchaseIntent {
        &self.intent
    }

    pub fn authority(&self) -> &RuntimePurchaseEffectAuthority {
        &self.authority
    }

    pub const fn wallet_address(&self) -> WalletAddress {
        self.wallet_address
    }

    pub fn wallet_binding(&self) -> &ValidatedChainOutcomeBindingV1 {
        &self.chain_outcome.binding
    }

    pub const fn transaction_hash(&self) -> Digest32 {
        self.chain_transaction
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeBuyReceipt {
    mint_id: Digest32,
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    chain_transaction: Digest32,
}

impl fmt::Debug for RuntimeBuyReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeBuyReceipt")
            .field("mint_id", &self.mint_id)
            .field("binding", &self.binding)
            .field("action", &self.action)
            .field("chain_transaction", &self.chain_transaction)
            .finish()
    }
}

impl RuntimeBuyReceipt {
    pub const fn mint_id(&self) -> Digest32 {
        self.mint_id
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        self.binding.encrypted_content()
    }

    pub fn key_envelope(&self) -> &KeyEnvelopeIdentityV1 {
        self.binding.key_envelope()
    }

    pub const fn wallet(&self) -> WalletAddress {
        self.binding.wallet()
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub const fn chain_transaction(&self) -> Digest32 {
        self.chain_transaction
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimePreparedRecipient {
    audit_request_id: Digest32,
    prepared_recipient_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    binding: ProtectedContentBindingV1,
    action: RightsActionV1,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    expires_at: u64,
    recipient_public_key: RecipientPublicKeyBytesV1,
    recipient_identity: RecipientKeyIdentityV1,
}

impl fmt::Debug for RuntimePreparedRecipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimePreparedRecipient")
            .field("audit_request_id", &self.audit_request_id)
            .field("prepared_recipient_handle", &"[redacted]")
            .field("binding", &self.binding)
            .field("action", &self.action)
            .field("runtime_operation_issuer", &self.runtime_operation_issuer)
            .field("expires_at", &self.expires_at)
            .field("recipient_public_key", &self.recipient_public_key)
            .field("recipient_identity", &self.recipient_identity)
            .finish()
    }
}

impl RuntimePreparedRecipient {
    pub const fn audit_request_id(&self) -> Digest32 {
        self.audit_request_id
    }

    pub const fn prepared_recipient_handle(&self) -> &[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        &self.prepared_recipient_handle
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub const fn runtime_operation_issuer(&self) -> RuntimeOperationIssuerKeyV1 {
        self.runtime_operation_issuer
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub const fn recipient_public_key(&self) -> &RecipientPublicKeyBytesV1 {
        &self.recipient_public_key
    }

    pub const fn recipient_identity(&self) -> &RecipientKeyIdentityV1 {
        &self.recipient_identity
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeViewerSession {
    audit_request_id: Digest32,
    viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    encrypted_content: EncryptedContentIdentityV1,
    action: RightsActionV1,
    expires_at: u64,
}

impl fmt::Debug for RuntimeViewerSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeViewerSession")
            .field("audit_request_id", &self.audit_request_id)
            .field("viewer_session_handle", &"[redacted]")
            .field("encrypted_content", &self.encrypted_content)
            .field("action", &self.action)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl RuntimeViewerSession {
    pub const fn audit_request_id(&self) -> Digest32 {
        self.audit_request_id
    }

    pub const fn viewer_session_handle(&self) -> &[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        &self.viewer_session_handle
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub const fn action(&self) -> RightsActionV1 {
        self.action
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeViewerMediaPart {
    audit_request_id: Digest32,
    viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    part_selector: ViewerMediaPartSelectorV1,
    clear_media_part: Vec<u8>,
}

impl fmt::Debug for RuntimeViewerMediaPart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeViewerMediaPart")
            .field("audit_request_id", &self.audit_request_id)
            .field("viewer_session_handle", &"[redacted]")
            .field("part_selector", &self.part_selector)
            .field("clear_media_part_len", &self.clear_media_part.len())
            .finish()
    }
}

impl RuntimeViewerMediaPart {
    pub const fn audit_request_id(&self) -> Digest32 {
        self.audit_request_id
    }

    pub const fn viewer_session_handle(&self) -> &[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        &self.viewer_session_handle
    }

    pub const fn part_selector(&self) -> &ViewerMediaPartSelectorV1 {
        &self.part_selector
    }

    pub fn clear_media_part(&self) -> &[u8] {
        &self.clear_media_part
    }
}

pub fn reject_bearer_playback(bytes: &[u8]) -> Result<(), RuntimeOpenError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| RuntimeOpenError::DecryptResult)?;
    if json_contains_bearer_playback(&value) {
        return Err(RuntimeOpenError::BearerPlaybackUrl);
    }
    Ok(())
}

pub fn bind_buy(
    custody_provisioned_mint: &PersistedRuntimeMint,
    wallet_request_bytes: &[u8],
    wallet_response_bytes: &[u8],
    purchase_effect: &RuntimeVerifiedPurchaseEffect,
    now_unix_seconds: u64,
) -> Result<RuntimeBuyReceipt, RuntimeOpenError> {
    reject_bearer_playback(wallet_request_bytes)?;
    reject_bearer_playback(wallet_response_bytes)?;
    if looks_like_home_launch_token(wallet_request_bytes) {
        return Err(RuntimeOpenError::WalletAuthority);
    }
    if custody_provisioned_mint.custody_terminal()
        != Some(crate::RuntimeCustodyTerminalKind::CustodyProvisioned)
        || custody_provisioned_mint.content_availability().is_none()
    {
        return Err(RuntimeOpenError::MintSelection);
    }
    let wallet_request = WalletProviderRequestV2::decode_at(wallet_request_bytes, now_unix_seconds)
        .map_err(|_| RuntimeOpenError::WalletAuthority)?;
    let wallet_response =
        WalletProviderResponseV2::decode_for_request(wallet_response_bytes, &wallet_request)
            .map_err(|_| RuntimeOpenError::WalletAuthority)?;
    let (account_id, expected) = expected_rights_request_from_wallet(&wallet_request)?;
    if wallet_request.authority.principal_id != purchase_effect.authority().principal_id() {
        return Err(RuntimeOpenError::ChainEvidence);
    }
    if account_id != purchase_effect.authority().account_id() {
        return Err(RuntimeOpenError::ChainEvidence);
    }
    if purchase_effect.intent().mint_id() != custody_provisioned_mint.draft().mint_id()
        || purchase_effect.intent().encrypted_content()
            != custody_provisioned_mint.draft().encrypted_content()
        || purchase_effect.intent().key_envelope()
            != custody_provisioned_mint.draft().key_envelope()
        || purchase_effect.intent().rights_policy() != custody_provisioned_mint.draft().policy()
        || purchase_effect.intent().encrypted_content() != expected.binding().encrypted_content()
        || purchase_effect.intent().key_envelope() != expected.binding().key_envelope()
        || purchase_effect.intent().rights_policy() != expected.binding().rights_policy()
        || purchase_effect.intent().action() != expected.action()
    {
        return Err(RuntimeOpenError::MintSelection);
    }
    let signed = validate_wallet_rights_signature(&wallet_request, &wallet_response, &expected)?;
    if signed.request().binding().wallet() != purchase_effect.wallet_address() {
        return Err(RuntimeOpenError::ChainEvidence);
    }
    Ok(RuntimeBuyReceipt {
        mint_id: custody_provisioned_mint.draft().mint_id(),
        binding: signed.request().binding().clone(),
        action: signed.request().action(),
        chain_transaction: purchase_effect.transaction_hash(),
    })
}

pub async fn prepare_recipient(
    decrypt: &dyn RuntimeDecryptProvider,
    buy: &RuntimeBuyReceipt,
    audit_request_id: RuntimeReleaseAuditIdV1,
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    now_unix_seconds: u64,
    expires_at: u64,
) -> Result<RuntimePreparedRecipient, RuntimeOpenError> {
    let request = DecryptProviderRequestV1::new_prepare_recipient(
        buy.binding(),
        audit_request_id,
        buy.action(),
        runtime_operation_issuer,
        now_unix_seconds,
        expires_at,
    )?;
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&request_bytes)?;
    ValidatedDecryptProviderRequestV1::decode_and_validate_at(
        &request_bytes,
        runtime_operation_issuer,
        now_unix_seconds,
    )
    .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response = decrypt
        .prepare_recipient(&request)
        .await
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response_bytes = response
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&response_bytes)?;
    if response.status() != DecryptProviderResponseStatusV1::PreparedRecipient {
        return Err(RuntimeOpenError::DecryptResult);
    }
    if response.audit_request_id()? != audit_request_id {
        return Err(RuntimeOpenError::DecryptResult);
    }
    Ok(RuntimePreparedRecipient {
        audit_request_id: audit_request_id.digest(),
        prepared_recipient_handle: *response.prepared_recipient_handle()?,
        binding: buy.binding().clone(),
        action: buy.action(),
        runtime_operation_issuer,
        expires_at,
        recipient_public_key: response.recipient_public_key()?,
        recipient_identity: response.recipient_identity()?,
    })
}

pub async fn open_viewer_session(
    decrypt: &dyn RuntimeDecryptProvider,
    input: &RuntimeOpenViewerSessionInput<'_>,
) -> Result<RuntimeViewerSession, RuntimeOpenError> {
    let binding = input
        .signed_runtime_release_operation
        .statement()
        .rights_request()
        .request()
        .binding();
    if input.prepared_recipient.binding() != input.buy.binding()
        || input.prepared_recipient.action() != input.buy.action()
        || input.prepared_recipient.audit_request_id()
            != input
                .signed_runtime_release_operation
                .statement()
                .audit_request_id()
                .digest()
        || input.prepared_recipient.runtime_operation_issuer()
            != input
                .signed_runtime_release_operation
                .statement()
                .runtime_operation_issuer()
        || input.prepared_recipient.recipient_public_key()
            != &input
                .signed_runtime_release_operation
                .statement()
                .recipient_public_key()
        || input.prepared_recipient.recipient_identity()
            != input
                .signed_runtime_release_operation
                .statement()
                .recipient_authorization()
                .statement()
                .recipient_identity()
        || input.now_unix_seconds >= input.prepared_recipient.expires_at()
        || input
            .signed_runtime_release_operation
            .statement()
            .expires_at()
            > input.prepared_recipient.expires_at()
        || input.buy.binding() != binding
        || input.buy.action()
            != input
                .signed_runtime_release_operation
                .statement()
                .rights_request()
                .request()
                .action()
    {
        return Err(RuntimeOpenError::MintSelection);
    }
    let request = DecryptProviderRequestV1::new_open_viewer_session(
        *input.prepared_recipient.prepared_recipient_handle(),
        input.signed_runtime_release_operation,
        input.expected_terminal_issuer,
        input.custody_envelope,
        input.media_identity,
        input.protected_init_segment,
        input.signed_node_contributions,
        input.signed_terminal_receipt,
    )?;
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&request_bytes)?;
    ValidatedDecryptProviderRequestV1::decode_and_validate_at(
        &request_bytes,
        input
            .signed_runtime_release_operation
            .statement()
            .runtime_operation_issuer(),
        input.now_unix_seconds,
    )
    .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response = decrypt
        .open_viewer_session(&request)
        .await
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response_bytes = response
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&response_bytes)?;
    if response.status() != DecryptProviderResponseStatusV1::ViewerSessionOpened {
        return Err(RuntimeOpenError::DecryptResult);
    }
    let audit_request_id = input
        .signed_runtime_release_operation
        .statement()
        .audit_request_id();
    let response_audit_request_id = response
        .audit_request_id()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    if response_audit_request_id != audit_request_id {
        return Err(RuntimeOpenError::DecryptResult);
    }
    let handle = *response
        .viewer_session_handle()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    Ok(RuntimeViewerSession {
        audit_request_id: audit_request_id.digest(),
        viewer_session_handle: handle,
        encrypted_content: binding.encrypted_content().clone(),
        action: input.buy.action(),
        expires_at: input
            .signed_runtime_release_operation
            .statement()
            .expires_at(),
    })
}

pub async fn cancel_prepared_recipient(
    decrypt: &dyn RuntimeDecryptProvider,
    prepared: &RuntimePreparedRecipient,
) -> Result<(), RuntimeOpenError> {
    let audit_request_id = RuntimeReleaseAuditIdV1::new(prepared.audit_request_id)
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let request = DecryptProviderRequestV1::new_cancel_prepared_recipient(
        audit_request_id,
        prepared.prepared_recipient_handle,
    )?;
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&request_bytes)?;
    let response = decrypt
        .cancel_prepared_recipient(&request)
        .await
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response_bytes = response
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&response_bytes)?;
    match response.status() {
        DecryptProviderResponseStatusV1::CancelledPreparedRecipient
        | DecryptProviderResponseStatusV1::PreparedRecipientAlreadyAbsent => {}
        _ => return Err(RuntimeOpenError::DecryptResult),
    }
    if response.audit_request_id()? != audit_request_id
        || response.prepared_recipient_handle()? != &prepared.prepared_recipient_handle
    {
        return Err(RuntimeOpenError::DecryptResult);
    }
    Ok(())
}

pub async fn read_viewer_media_part(
    decrypt: &dyn RuntimeDecryptProvider,
    session: &RuntimeViewerSession,
    part_selector: ViewerMediaPartSelectorV1,
    now_unix_seconds: u64,
) -> Result<RuntimeViewerMediaPart, RuntimeOpenError> {
    if now_unix_seconds >= session.expires_at {
        return Err(RuntimeOpenError::DecryptResult);
    }
    let audit_request_id = RuntimeReleaseAuditIdV1::new(session.audit_request_id)
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let request = DecryptProviderRequestV1::new_read_viewer_media_part(
        audit_request_id,
        session.viewer_session_handle,
        part_selector.clone(),
    )?;
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&request_bytes)?;
    let response = decrypt
        .read_viewer_media_part(&request)
        .await
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response_bytes = response
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&response_bytes)?;
    if response.status() != DecryptProviderResponseStatusV1::ViewerMediaPart {
        return Err(RuntimeOpenError::DecryptResult);
    }
    if response.audit_request_id()? != audit_request_id
        || response.viewer_session_handle()? != &session.viewer_session_handle
        || response.viewer_media_part_selector()? != &part_selector
    {
        return Err(RuntimeOpenError::DecryptResult);
    }
    Ok(RuntimeViewerMediaPart {
        audit_request_id: audit_request_id.digest(),
        viewer_session_handle: session.viewer_session_handle,
        part_selector,
        clear_media_part: response.clear_media_part()?.to_vec(),
    })
}

pub async fn close_viewer_session(
    decrypt: &dyn RuntimeDecryptProvider,
    session: &RuntimeViewerSession,
) -> Result<(), RuntimeOpenError> {
    let audit_request_id = RuntimeReleaseAuditIdV1::new(session.audit_request_id)
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let request = DecryptProviderRequestV1::new_close_viewer_session(
        audit_request_id,
        session.viewer_session_handle,
    )?;
    let request_bytes = request
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&request_bytes)?;
    let response = decrypt
        .close_viewer_session(&request)
        .await
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    let response_bytes = response
        .to_json_vec()
        .map_err(|_| RuntimeOpenError::DecryptResult)?;
    reject_bearer_playback(&response_bytes)?;
    match response.status() {
        DecryptProviderResponseStatusV1::ClosedViewerSession
        | DecryptProviderResponseStatusV1::ViewerSessionAlreadyAbsent => {}
        _ => return Err(RuntimeOpenError::DecryptResult),
    }
    if response.audit_request_id()? != audit_request_id
        || response.viewer_session_handle()? != &session.viewer_session_handle
    {
        return Err(RuntimeOpenError::DecryptResult);
    }
    Ok(())
}

fn chain_transaction_digest(
    chain_outcome: &ValidatedChainOutcomeV1,
) -> Result<Digest32, RuntimeOpenError> {
    chain_outcome
        .validate()
        .map_err(|_| RuntimeOpenError::ChainEvidence)?;
    transaction_digest_from_hash(chain_outcome.transaction_hash.as_str())
}

fn transaction_digest_from_hash(transaction_hash: &str) -> Result<Digest32, RuntimeOpenError> {
    let hex = transaction_hash
        .strip_prefix("0x")
        .or_else(|| transaction_hash.strip_prefix("sha256:"))
        .ok_or(RuntimeOpenError::ChainEvidence)?;
    let bytes = hex::decode(hex).map_err(|_| RuntimeOpenError::ChainEvidence)?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| RuntimeOpenError::ChainEvidence)?;
    let digest = Digest32::new(array);
    if digest == Digest32::new([0; 32]) {
        return Err(RuntimeOpenError::ChainEvidence);
    }
    Ok(digest)
}

fn expected_rights_request_from_wallet(
    wallet_request: &WalletProviderRequestV2,
) -> Result<(String, RightsRequestV1), RuntimeOpenError> {
    let (account_id, canonical_rights_request_hex) = match &wallet_request.operation {
        WalletProviderOperationV2::RequestProtectedContentRightsSignature {
            account_id,
            canonical_rights_request_hex,
            ..
        } => (account_id.clone(), canonical_rights_request_hex),
        _ => return Err(RuntimeOpenError::WalletAuthority),
    };
    let rights_request_bytes =
        hex::decode(canonical_rights_request_hex).map_err(|_| RuntimeOpenError::WalletAuthority)?;
    let request = RightsRequestV1::from_canonical_bytes(&rights_request_bytes)
        .map_err(|_| RuntimeOpenError::WalletAuthority)?;
    Ok((account_id, request))
}

fn looks_like_home_launch_token(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    match value {
        Value::Object(map) => {
            map.contains_key("launch_token")
                || map.contains_key("home_launch_token")
                || map
                    .get("token_type")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("home_launch"))
        }
        _ => false,
    }
}

fn json_contains_bearer_playback(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "play_url" | "playback_url" | "playUrl" | "bearer_play_url"
                )
            }) || map.values().any(json_contains_bearer_playback)
        }
        Value::Array(items) => items.iter().any(json_contains_bearer_playback),
        Value::String(text) => {
            text.contains("play_url=")
                || text.contains("/play_url")
                || text.contains("playback_url=")
        }
        _ => false,
    }
}

fn validate_required_text(
    value: &str,
    max_bytes: usize,
    error: RuntimeOpenError,
) -> Result<(), RuntimeOpenError> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(error);
    }
    Ok(())
}

fn decode_prefixed_hex_bytes(value: &str, max_bytes: usize) -> Result<Vec<u8>, RuntimeOpenError> {
    let hex = value
        .strip_prefix("0x")
        .ok_or(RuntimeOpenError::ChainEvidence)?;
    if hex.len() > max_bytes.saturating_mul(2)
        || !hex.len().is_multiple_of(2)
        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RuntimeOpenError::ChainEvidence);
    }
    hex::decode(hex).map_err(|_| RuntimeOpenError::ChainEvidence)
}

fn validate_hex_quantity(value: &str, max_bytes: usize) -> Result<(), RuntimeOpenError> {
    let hex = value
        .strip_prefix("0x")
        .ok_or(RuntimeOpenError::ChainEvidence)?;
    if hex.is_empty()
        || hex.len() > max_bytes.saturating_mul(2)
        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RuntimeOpenError::ChainEvidence);
    }
    Ok(())
}

fn parse_wallet_address(value: &str) -> Result<WalletAddress, RuntimeOpenError> {
    let bytes = decode_prefixed_hex_bytes(value, 20)?;
    let array: [u8; 20] = bytes
        .try_into()
        .map_err(|_| RuntimeOpenError::ChainEvidence)?;
    Ok(WalletAddress::new(array))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use elastos_auth::ethereum_signed_message_hash;
    use elastos_protected_content_contracts::{
        CanonicalContract, ContentAccessIdV1, CustodyCommitteeAuthorizationIdentityV1,
        CustodyEpochIdentityV1, CustodyNodeProvisioningRecordIdentityV1,
        CustodyPoolFailureDomainIdV1, CustodyPoolIdentityV1, CustodyPoolOperatorIdV1,
        KeyEnvelopeIdentityV1, NodeSetV1, ProfileIdentityV1, ProtectedContentBindingV1,
        RecipientKeyIdentityV1, RecipientPublicKeyBytesV1, ReplayNonce16, RightsActionV1,
        RightsPolicyIdentityV1, RightsRequestV1, RuntimeCustodyProvisioningIdV1,
        RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeSessionBindingV1, ThresholdV1,
        WalletAddress, WalletSignedRightsRequestV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    };
    use elastos_wallet_contract::{
        ProtectedContentRightsSignatureResultV1, ValidatedChainOutcomeBindingV1,
        VerifiedWalletInvocationContext, WalletProviderOperationV2, WalletProviderRequestV2,
        WalletProviderResponseV2, WalletResultV2,
    };
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use serde_json::json;
    use sha3::{Digest as _, Keccak256};
    use tempfile::tempdir;
    use x_wing::kem::{Decapsulator as _, KeyExport as _};

    use super::*;
    use crate::coordinator::wallet_address_hex;
    use crate::test_media;
    use crate::{
        RuntimeContentAvailabilityRequirement, RuntimeMintDraft, RuntimeMintJournal,
        RuntimeMintNodeBinding, RuntimeMintNodeReceipt, RuntimeVerifiedContentAvailability,
    };

    const NOW: u64 = 2_000_000_000;
    const WALLET_ACCOUNT: &str = "wallet-account-alpha";

    fn digest(byte: u8) -> Digest32 {
        Digest32::new([byte; 32])
    }

    fn content_access_id(seed: u8) -> ContentAccessIdV1 {
        ContentAccessIdV1::new([seed; 16]).unwrap()
    }

    fn node_public_key(seed: u8) -> elastos_protected_content_contracts::NodePublicKey {
        elastos_protected_content_contracts::NodePublicKey::new(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap()
    }

    fn wallet_key(seed: u8) -> WalletSigningKey {
        WalletSigningKey::from_slice(&[seed; 32]).unwrap()
    }

    fn wallet(seed: u8) -> WalletAddress {
        let encoded = wallet_key(seed).verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
        WalletAddress::new(digest[12..].try_into().unwrap())
    }

    fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
        RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(seed.max(9)))
            .unwrap()
            .key_identity(CUSTODY_X_WING_AES256GCM_SUITE_ID_V1)
            .unwrap()
    }

    fn xwing_public_key_bytes(
        seed: u8,
    ) -> [u8; elastos_protected_content_contracts::PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] {
        let secret = x_wing::DecapsulationKey::from([seed; x_wing::DECAPSULATION_KEY_SIZE]);
        secret.encapsulation_key().to_bytes().into()
    }

    fn mint_binding(seed: u8) -> RuntimeMintNodeBinding {
        RuntimeMintNodeBinding::new(
            node_public_key(seed),
            CustodyPoolOperatorIdV1::new([0x80 + seed; 32]),
            CustodyPoolFailureDomainIdV1::new([0x90 + seed; 32]),
            digest(0xa0 + seed),
        )
        .unwrap()
    }

    fn mint_draft() -> RuntimeMintDraft {
        let nodes = vec![mint_binding(1), mint_binding(2), mint_binding(3)];
        let threshold = ThresholdV1::new(2, 3).unwrap();
        let (init_segment, encrypted_segments, mime_type, codecs) =
            test_media::media_components(0x11);
        let encrypted = test_media::media_identity(0x11).encrypted_content().clone();
        let node_set = NodeSetV1::new(
            threshold,
            nodes.iter().map(|node| node.node_public_key()).collect(),
        )
        .unwrap();
        let key_envelope = KeyEnvelopeIdentityV1::new(
            encrypted.clone(),
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

    fn mint_receipt(node: &RuntimeMintNodeBinding, seed: u8) -> RuntimeMintNodeReceipt {
        RuntimeMintNodeReceipt::new(
            node.node_public_key(),
            RuntimeCustodyProvisioningIdV1::new(digest(seed)).unwrap(),
            CustodyNodeProvisioningRecordIdentityV1::new(digest(seed ^ 0x21), 128).unwrap(),
            node.owner_state_root(),
        )
        .unwrap()
    }

    fn create_owner_only_directory(path: &std::path::Path) {
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    fn persist_mint(custody_provisioned: bool) -> (tempfile::TempDir, PersistedRuntimeMint) {
        let temp = tempdir().unwrap();
        let parent = temp.path().join("owner-only-parent");
        create_owner_only_directory(&parent);
        let journal = RuntimeMintJournal::new(parent.join("runtime-mint"));
        let draft = mint_draft();
        journal.persist_bound(&draft).unwrap();
        if custody_provisioned {
            for (index, node) in draft.nodes().iter().enumerate() {
                journal
                    .mark_node_effect_started(draft.mint_id(), node.node_public_key())
                    .unwrap();
                journal
                    .mark_node_receipt(draft.mint_id(), mint_receipt(node, 0x80 + index as u8))
                    .unwrap();
            }
            let provisioned = journal.mark_custody_provisioned(draft.mint_id()).unwrap();
            (temp, provisioned)
        } else {
            let bound = journal.load(draft.mint_id()).unwrap();
            (temp, bound)
        }
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

    fn persist_content_availability(
        temp: &tempfile::TempDir,
        provisioned: &PersistedRuntimeMint,
    ) -> PersistedRuntimeMint {
        let requirement = availability_requirement();
        let evidence = RuntimeVerifiedContentAvailability::new(
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#content",
            "did:key:z6Mkhq7f4c4QAEgwRByrEsmGu3RJRYvpP5UGcWvqBjGW4YRe#publisher",
            &requirement,
            3,
            NOW,
            digest(0x7e),
            provisioned.draft().encrypted_content().clone(),
            provisioned.draft().media_identity().media_manifest_root(),
        )
        .unwrap();
        RuntimeMintJournal::new(temp.path().join("owner-only-parent").join("runtime-mint"))
            .mark_content_available(provisioned.draft().mint_id(), &requirement, evidence)
            .unwrap()
    }

    fn wallet_for_policy(
        mint: &PersistedRuntimeMint,
        policy: RightsPolicyIdentityV1,
    ) -> (Vec<u8>, Vec<u8>) {
        let binding = ProtectedContentBindingV1::new(
            mint.draft().encrypted_content().clone(),
            mint.draft().key_envelope().clone(),
            policy,
            ProfileIdentityV1::from_public_key_bytes(
                SigningKey::from_bytes(&[0x26; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
            wallet(7),
            RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
        )
        .unwrap();
        let request = RightsRequestV1::new(
            binding,
            RightsActionV1::View,
            recipient_identity(0x30),
            NOW,
            NOW + 180,
            ReplayNonce16::new([0x55; 16]),
        )
        .unwrap();
        let (signature, recovery_id) = wallet_key(7)
            .sign_prehash_recoverable(&ethereum_signed_message_hash(
                &request.canonical_bytes().unwrap(),
            ))
            .unwrap();
        let mut signature_bytes = signature.to_bytes().to_vec();
        signature_bytes.push(recovery_id.to_byte());
        let signed = WalletSignedRightsRequestV1::new(request.clone(), signature_bytes).unwrap();
        let context = VerifiedWalletInvocationContext::new(
            "profile:alpha",
            "runtime-session:alpha",
            Some("proof:alpha".to_string()),
            "grant:alpha",
            "runtime",
            "launch:alpha",
        )
        .unwrap();
        let wallet_request = WalletProviderRequestV2::new(
            &context,
            "wallet-request:11111111111111111111111111111111",
            NOW,
            NOW + 120,
            WalletProviderOperationV2::RequestProtectedContentRightsSignature {
                account_id: WALLET_ACCOUNT.to_string(),
                canonical_rights_request_hex: hex::encode(request.canonical_bytes().unwrap()),
                reason: "Open protected content".to_string(),
            },
        )
        .unwrap();
        let result = ProtectedContentRightsSignatureResultV1::new(
            WALLET_ACCOUNT,
            wallet_address_hex(request.binding().wallet()),
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
            serde_json::to_vec(&wallet_request).unwrap(),
            serde_json::to_vec(&wallet_response).unwrap(),
        )
    }

    fn purchase_intent(mint: &PersistedRuntimeMint) -> RuntimeProtectedContentPurchaseIntent {
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
        .unwrap()
    }

    fn purchase_effect(
        mint: &PersistedRuntimeMint,
        account_id: &str,
        tx_byte: u8,
    ) -> RuntimeVerifiedPurchaseEffect {
        purchase_effect_for_principal(mint, "profile:alpha", account_id, tx_byte)
    }

    fn buy_receipt_for_open_tests(mint: &PersistedRuntimeMint) -> RuntimeBuyReceipt {
        let (request_bytes, response_bytes) =
            wallet_for_policy(mint, mint.draft().policy().clone());
        let request = WalletProviderRequestV2::decode_at(&request_bytes, NOW).unwrap();
        let response =
            WalletProviderResponseV2::decode_for_request(&response_bytes, &request).unwrap();
        let (_, expected) = expected_rights_request_from_wallet(&request).unwrap();
        let signed = validate_wallet_rights_signature(&request, &response, &expected).unwrap();
        RuntimeBuyReceipt {
            mint_id: mint.draft().mint_id(),
            binding: signed.request().binding().clone(),
            action: signed.request().action(),
            chain_transaction: digest(0xaa),
        }
    }

    fn purchase_effect_for_principal(
        mint: &PersistedRuntimeMint,
        principal_id: &str,
        account_id: &str,
        tx_byte: u8,
    ) -> RuntimeVerifiedPurchaseEffect {
        RuntimeVerifiedPurchaseEffect::new(
            purchase_intent(mint),
            RuntimePurchaseEffectAuthority::new(
                principal_id,
                account_id,
                wallet_address_hex(wallet(7)),
                "wallet-request:00112233445566778899aabbccddeeff",
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
            NOW,
        )
        .unwrap()
    }

    fn runtime_operation_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
        RuntimeOperationIssuerKeyV1::new(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap()
    }

    fn runtime_release_audit_id(seed: u8) -> RuntimeReleaseAuditIdV1 {
        RuntimeReleaseAuditIdV1::new(digest(seed)).unwrap()
    }

    fn opaque_handle(seed: u8) -> [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        bytes[0] = seed.max(1);
        bytes[31] = seed ^ 0x5a;
        bytes
    }

    fn viewer_session(seed: u8) -> RuntimeViewerSession {
        RuntimeViewerSession {
            audit_request_id: digest(0x80 ^ seed),
            viewer_session_handle: opaque_handle(seed),
            encrypted_content: test_media::media_identity(0x11).encrypted_content().clone(),
            action: RightsActionV1::View,
            expires_at: NOW + 60,
        }
    }

    struct FakeDecryptProvider {
        expected_issuer: RuntimeOperationIssuerKeyV1,
        now: u64,
        prepare_response: Result<DecryptProviderResponseV1, RuntimeProviderCallError>,
        open_response: Result<DecryptProviderResponseV1, RuntimeProviderCallError>,
        read_response: Result<DecryptProviderResponseV1, RuntimeProviderCallError>,
        cancel_response: Result<DecryptProviderResponseV1, RuntimeProviderCallError>,
        close_response: Result<DecryptProviderResponseV1, RuntimeProviderCallError>,
        prepare_requests: std::sync::Mutex<Vec<Vec<u8>>>,
        read_requests: std::sync::Mutex<Vec<Vec<u8>>>,
        cancel_requests: std::sync::Mutex<Vec<Vec<u8>>>,
        close_requests: std::sync::Mutex<Vec<Vec<u8>>>,
    }

    impl FakeDecryptProvider {
        fn with_prepare_response(
            expected_issuer: RuntimeOperationIssuerKeyV1,
            now: u64,
            response: DecryptProviderResponseV1,
        ) -> Self {
            Self {
                expected_issuer,
                now,
                prepare_response: Ok(response),
                open_response: Err(RuntimeProviderCallError::NoExactResult),
                read_response: Err(RuntimeProviderCallError::NoExactResult),
                cancel_response: Err(RuntimeProviderCallError::NoExactResult),
                close_response: Err(RuntimeProviderCallError::NoExactResult),
                prepare_requests: std::sync::Mutex::new(Vec::new()),
                read_requests: std::sync::Mutex::new(Vec::new()),
                cancel_requests: std::sync::Mutex::new(Vec::new()),
                close_requests: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn with_read_response(response: DecryptProviderResponseV1) -> Self {
            Self {
                expected_issuer: runtime_operation_issuer(0x42),
                now: NOW,
                prepare_response: Err(RuntimeProviderCallError::NoExactResult),
                open_response: Err(RuntimeProviderCallError::NoExactResult),
                read_response: Ok(response),
                cancel_response: Err(RuntimeProviderCallError::NoExactResult),
                close_response: Err(RuntimeProviderCallError::NoExactResult),
                prepare_requests: std::sync::Mutex::new(Vec::new()),
                read_requests: std::sync::Mutex::new(Vec::new()),
                cancel_requests: std::sync::Mutex::new(Vec::new()),
                close_requests: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn with_cancel_response(response: DecryptProviderResponseV1) -> Self {
            Self {
                expected_issuer: runtime_operation_issuer(0x42),
                now: NOW,
                prepare_response: Err(RuntimeProviderCallError::NoExactResult),
                open_response: Err(RuntimeProviderCallError::NoExactResult),
                read_response: Err(RuntimeProviderCallError::NoExactResult),
                cancel_response: Ok(response),
                close_response: Err(RuntimeProviderCallError::NoExactResult),
                prepare_requests: std::sync::Mutex::new(Vec::new()),
                read_requests: std::sync::Mutex::new(Vec::new()),
                cancel_requests: std::sync::Mutex::new(Vec::new()),
                close_requests: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn with_close_response(response: DecryptProviderResponseV1) -> Self {
            Self {
                expected_issuer: runtime_operation_issuer(0x42),
                now: NOW,
                prepare_response: Err(RuntimeProviderCallError::NoExactResult),
                open_response: Err(RuntimeProviderCallError::NoExactResult),
                read_response: Err(RuntimeProviderCallError::NoExactResult),
                cancel_response: Err(RuntimeProviderCallError::NoExactResult),
                close_response: Ok(response),
                prepare_requests: std::sync::Mutex::new(Vec::new()),
                read_requests: std::sync::Mutex::new(Vec::new()),
                cancel_requests: std::sync::Mutex::new(Vec::new()),
                close_requests: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl RuntimeDecryptProvider for FakeDecryptProvider {
        async fn prepare_recipient(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let bytes = request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            self.prepare_requests.lock().unwrap().push(bytes.clone());
            ValidatedDecryptProviderRequestV1::decode_and_validate_at(
                &bytes,
                self.expected_issuer,
                self.now,
            )
            .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            self.prepare_response.clone()
        }

        async fn open_viewer_session(
            &self,
            _request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            self.open_response.clone()
        }

        async fn read_viewer_media_part(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let bytes = request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            self.read_requests.lock().unwrap().push(bytes);
            self.read_response.clone()
        }

        async fn cancel_prepared_recipient(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let bytes = request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            self.cancel_requests.lock().unwrap().push(bytes);
            self.cancel_response.clone()
        }

        async fn close_viewer_session(
            &self,
            request: &DecryptProviderRequestV1,
        ) -> Result<DecryptProviderResponseV1, RuntimeProviderCallError> {
            let bytes = request
                .to_json_vec()
                .map_err(|_| RuntimeProviderCallError::NoExactResult)?;
            self.close_requests.lock().unwrap().push(bytes);
            self.close_response.clone()
        }
    }

    #[test]
    fn bearer_play_url_is_rejected() {
        assert_eq!(
            reject_bearer_playback(br#"{"play_url":"https://example.test/play"}"#),
            Err(RuntimeOpenError::BearerPlaybackUrl)
        );
        assert_eq!(
            reject_bearer_playback(br#"{"viewer":{"playback_url":"https://example.test/play"}}"#),
            Err(RuntimeOpenError::BearerPlaybackUrl)
        );
        reject_bearer_playback(br#"{"viewer_session_handle":"aa","schema":"ok"}"#).unwrap();
    }

    #[test]
    fn home_launch_token_is_not_wallet_authority() {
        let token = br#"{"launch_token":"home-edge-credential","op":"open"}"#;
        assert!(looks_like_home_launch_token(token));
        assert!(!looks_like_home_launch_token(
            br#"{"schema":"elastos.wallet.provider.request/v2"}"#
        ));
    }

    #[test]
    fn bind_buy_rejects_custody_provisioned_mint_pending_availability_receipt() {
        let (_temp, provisioned) = persist_mint(true);
        let (request, response) =
            wallet_for_policy(&provisioned, provisioned.draft().policy().clone());
        let effect = purchase_effect(&provisioned, WALLET_ACCOUNT, 0xaa);
        assert_eq!(
            bind_buy(&provisioned, &request, &response, &effect, NOW),
            Err(RuntimeOpenError::MintSelection)
        );
    }

    #[test]
    fn bind_buy_accepts_exact_verified_content_availability() {
        let (temp, provisioned) = persist_mint(true);
        let available = persist_content_availability(&temp, &provisioned);
        let (request, response) = wallet_for_policy(&available, available.draft().policy().clone());
        let effect = purchase_effect(&available, WALLET_ACCOUNT, 0xaa);
        let receipt = bind_buy(&available, &request, &response, &effect, NOW).unwrap();
        assert_eq!(receipt.mint_id(), available.draft().mint_id());
        assert_eq!(
            receipt.binding().encrypted_content(),
            available.draft().encrypted_content()
        );
    }

    #[test]
    fn bind_buy_fails_closed_without_content_availability_and_on_wrong_inputs() {
        let (_bound_temp, bound) = persist_mint(false);
        let (request, response) = wallet_for_policy(&bound, bound.draft().policy().clone());
        let outcome = purchase_effect(&bound, WALLET_ACCOUNT, 0xaa);
        assert_eq!(
            bind_buy(&bound, &request, &response, &outcome, NOW),
            Err(RuntimeOpenError::MintSelection)
        );

        let (mint_temp, custody_provisioned) = persist_mint(true);
        let available_mint = persist_content_availability(&mint_temp, &custody_provisioned);
        let (request, response) = wallet_for_policy(
            &available_mint,
            RightsPolicyIdentityV1::new(digest(0x77), 384).unwrap(),
        );
        assert_eq!(
            bind_buy(&available_mint, &request, &response, &outcome, NOW),
            Err(RuntimeOpenError::MintSelection)
        );

        let context = VerifiedWalletInvocationContext::new(
            "profile:alpha",
            "runtime-session:alpha",
            Some("proof:alpha".to_string()),
            "grant:alpha",
            "runtime",
            "launch:alpha",
        )
        .unwrap();
        let generic = WalletProviderRequestV2::new(
            &context,
            "wallet-request:11111111111111111111111111111111",
            NOW,
            NOW + 120,
            WalletProviderOperationV2::ListAccounts {
                include_revoked: false,
            },
        )
        .unwrap();
        let generic_response = WalletProviderResponseV2::for_request(
            &generic,
            WalletResultV2::Ok {
                data: json!({"accounts": []}),
            },
        );
        assert_eq!(
            bind_buy(
                &available_mint,
                &serde_json::to_vec(&generic).unwrap(),
                &serde_json::to_vec(&generic_response).unwrap(),
                &outcome,
                NOW
            ),
            Err(RuntimeOpenError::WalletAuthority)
        );
    }

    #[test]
    fn bind_buy_fails_closed_on_bearer_url_home_token_and_invalid_chain() {
        let (temp, custody_provisioned) = persist_mint(true);
        let available_mint = persist_content_availability(&temp, &custody_provisioned);
        let (request, response) =
            wallet_for_policy(&available_mint, available_mint.draft().policy().clone());
        let outcome = purchase_effect(&available_mint, WALLET_ACCOUNT, 0xaa);

        let mut injected: serde_json::Value = serde_json::from_slice(&request).unwrap();
        injected["play_url"] = json!("https://example.test/play");
        assert_eq!(
            bind_buy(
                &available_mint,
                &serde_json::to_vec(&injected).unwrap(),
                &response,
                &outcome,
                NOW
            ),
            Err(RuntimeOpenError::BearerPlaybackUrl)
        );

        assert_eq!(
            bind_buy(
                &available_mint,
                br#"{"launch_token":"home-edge-credential","op":"open"}"#,
                br#"{"ok":true}"#,
                &outcome,
                NOW
            ),
            Err(RuntimeOpenError::WalletAuthority)
        );

        assert_eq!(
            RuntimeVerifiedPurchaseEffect::new(
                purchase_intent(&available_mint),
                RuntimePurchaseEffectAuthority::new(
                    "profile:alpha",
                    WALLET_ACCOUNT,
                    wallet_address_hex(wallet(7)),
                    "wallet-request:00112233445566778899aabbccddeeff",
                )
                .unwrap(),
                ValidatedChainOutcomeBindingV1::ManagedSigned {
                    signed_transaction_sha256: format!("sha256:{}", hex::encode([1; 32])),
                },
                format!("0x{}", hex::encode([0; 32])),
                json!({
                    "schema": "elastos.chain.broadcast_receipt/v1",
                    "network": "esc-mainnet",
                }),
                NOW,
            ),
            Err(RuntimeOpenError::ChainEvidence)
        );

        let mismatched = purchase_effect(&available_mint, "wallet-account-other", 0xaa);
        assert_eq!(
            bind_buy(&available_mint, &request, &response, &mismatched, NOW),
            Err(RuntimeOpenError::ChainEvidence)
        );

        let wrong_principal =
            purchase_effect_for_principal(&available_mint, "profile:beta", WALLET_ACCOUNT, 0xaa);
        assert_eq!(
            bind_buy(&available_mint, &request, &response, &wrong_principal, NOW),
            Err(RuntimeOpenError::ChainEvidence)
        );

        assert_eq!(
            RuntimeVerifiedPurchaseEffect::new(
                purchase_intent(&available_mint),
                RuntimePurchaseEffectAuthority::new(
                    "profile:alpha",
                    WALLET_ACCOUNT,
                    wallet_address_hex(wallet(7)),
                    "wallet-request:00112233445566778899aabbccddeeff",
                )
                .unwrap(),
                ValidatedChainOutcomeBindingV1::ManagedSigned {
                    signed_transaction_sha256: format!("sha256:{}", hex::encode([0xab; 32])),
                },
                format!("0x{}", hex::encode([0xaa; 32])),
                json!({"play_url":"https://example.test/play"}),
                NOW,
            ),
            Err(RuntimeOpenError::BearerPlaybackUrl)
        );
    }

    #[tokio::test]
    async fn prepare_recipient_returns_exact_typed_result_and_replays() {
        let (_temp, custody_provisioned_mint) = persist_mint(true);
        let buy = buy_receipt_for_open_tests(&custody_provisioned_mint);
        let audit_id = runtime_release_audit_id(0x71);
        let issuer = runtime_operation_issuer(0x42);
        let response = DecryptProviderResponseV1::new_prepared_recipient(
            audit_id,
            opaque_handle(0x21),
            RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(0x30)).unwrap(),
            &recipient_identity(0x30),
        )
        .unwrap();
        let provider = FakeDecryptProvider::with_prepare_response(issuer, NOW, response);

        let first = prepare_recipient(&provider, &buy, audit_id, issuer, NOW, NOW + 50)
            .await
            .unwrap();
        let second = prepare_recipient(&provider, &buy, audit_id, issuer, NOW, NOW + 50)
            .await
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.audit_request_id(), audit_id.digest());
        assert_eq!(first.prepared_recipient_handle(), &opaque_handle(0x21));
        assert_eq!(
            first.recipient_public_key(),
            &RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(0x30)).unwrap()
        );
        assert_eq!(first.recipient_identity(), &recipient_identity(0x30));
        assert_eq!(provider.prepare_requests.lock().unwrap().len(), 2);
        assert!(format!("{first:?}").contains("prepared_recipient_handle"));
        assert!(!format!("{first:?}").contains(&hex::encode(opaque_handle(0x21))));
    }

    #[tokio::test]
    async fn prepare_recipient_rejects_conflicting_response_binding() {
        let (_temp, custody_provisioned_mint) = persist_mint(true);
        let buy = buy_receipt_for_open_tests(&custody_provisioned_mint);
        let audit_id = runtime_release_audit_id(0x71);
        let issuer = runtime_operation_issuer(0x42);
        let wrong_response = DecryptProviderResponseV1::new_prepared_recipient(
            runtime_release_audit_id(0x72),
            opaque_handle(0x21),
            RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(0x30)).unwrap(),
            &recipient_identity(0x30),
        )
        .unwrap();
        let provider = FakeDecryptProvider::with_prepare_response(issuer, NOW, wrong_response);

        assert_eq!(
            prepare_recipient(&provider, &buy, audit_id, issuer, NOW, NOW + 50).await,
            Err(RuntimeOpenError::DecryptResult)
        );
    }

    #[tokio::test]
    async fn read_viewer_media_part_returns_exact_clear_part_and_replays() {
        let session = viewer_session(0x31);
        let selector =
            ViewerMediaPartSelectorV1::segment(1, test_media::media_components(0x11).1[1].clone())
                .unwrap();
        let response = DecryptProviderResponseV1::new_viewer_media_part(
            RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
            *session.viewer_session_handle(),
            selector.clone(),
            vec![0x10, 0x11, 0x12],
        )
        .unwrap();
        let provider = FakeDecryptProvider::with_read_response(response);

        let first = read_viewer_media_part(&provider, &session, selector.clone(), NOW)
            .await
            .unwrap();
        let second = read_viewer_media_part(&provider, &session, selector.clone(), NOW)
            .await
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.audit_request_id(), session.audit_request_id());
        assert_eq!(
            first.viewer_session_handle(),
            session.viewer_session_handle()
        );
        assert_eq!(first.part_selector(), &selector);
        assert_eq!(first.clear_media_part(), &[0x10, 0x11, 0x12]);
        assert_eq!(provider.read_requests.lock().unwrap().len(), 2);
        assert!(format!("{first:?}").contains("clear_media_part_len"));
        assert!(!format!("{first:?}").contains("101112"));
    }

    #[tokio::test]
    async fn read_viewer_media_part_rejects_conflicting_response_binding() {
        let session = viewer_session(0x31);
        let selector =
            ViewerMediaPartSelectorV1::segment(1, test_media::media_components(0x11).1[1].clone())
                .unwrap();
        let wrong_response = DecryptProviderResponseV1::new_viewer_media_part(
            RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
            opaque_handle(0x32),
            selector.clone(),
            vec![0x10, 0x11, 0x12],
        )
        .unwrap();
        let provider = FakeDecryptProvider::with_read_response(wrong_response);

        assert_eq!(
            read_viewer_media_part(&provider, &session, selector, NOW).await,
            Err(RuntimeOpenError::DecryptResult)
        );
    }

    #[tokio::test]
    async fn read_viewer_media_part_rejects_expired_session_before_provider_dispatch() {
        let session = RuntimeViewerSession {
            expires_at: NOW,
            ..viewer_session(0x31)
        };
        let selector =
            ViewerMediaPartSelectorV1::segment(1, test_media::media_components(0x11).1[1].clone())
                .unwrap();
        let response = DecryptProviderResponseV1::new_viewer_media_part(
            RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap(),
            *session.viewer_session_handle(),
            selector.clone(),
            vec![0x10, 0x11, 0x12],
        )
        .unwrap();
        let provider = FakeDecryptProvider::with_read_response(response);

        assert_eq!(
            read_viewer_media_part(&provider, &session, selector, NOW).await,
            Err(RuntimeOpenError::DecryptResult)
        );
        assert_eq!(provider.read_requests.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn cancel_prepared_recipient_accepts_exact_replay_and_already_absent() {
        let (_temp, custody_provisioned_mint) = persist_mint(true);
        let buy = buy_receipt_for_open_tests(&custody_provisioned_mint);
        let audit_id = runtime_release_audit_id(0x71);
        let issuer = runtime_operation_issuer(0x42);
        let prepare_response = DecryptProviderResponseV1::new_prepared_recipient(
            audit_id,
            opaque_handle(0x21),
            RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(0x30)).unwrap(),
            &recipient_identity(0x30),
        )
        .unwrap();
        let prepare_provider =
            FakeDecryptProvider::with_prepare_response(issuer, NOW, prepare_response);
        let prepared = prepare_recipient(&prepare_provider, &buy, audit_id, issuer, NOW, NOW + 50)
            .await
            .unwrap();

        let cancelled = FakeDecryptProvider::with_cancel_response(
            DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                audit_id,
                *prepared.prepared_recipient_handle(),
            )
            .unwrap(),
        );
        cancel_prepared_recipient(&cancelled, &prepared)
            .await
            .unwrap();
        cancel_prepared_recipient(&cancelled, &prepared)
            .await
            .unwrap();
        assert_eq!(cancelled.cancel_requests.lock().unwrap().len(), 2);

        let absent = FakeDecryptProvider::with_cancel_response(
            DecryptProviderResponseV1::new_prepared_recipient_already_absent(
                audit_id,
                *prepared.prepared_recipient_handle(),
            )
            .unwrap(),
        );
        cancel_prepared_recipient(&absent, &prepared).await.unwrap();
    }

    #[tokio::test]
    async fn cancel_prepared_recipient_rejects_conflicting_response_binding() {
        let (_temp, custody_provisioned_mint) = persist_mint(true);
        let buy = buy_receipt_for_open_tests(&custody_provisioned_mint);
        let audit_id = runtime_release_audit_id(0x71);
        let issuer = runtime_operation_issuer(0x42);
        let prepare_response = DecryptProviderResponseV1::new_prepared_recipient(
            audit_id,
            opaque_handle(0x21),
            RecipientPublicKeyBytesV1::new(xwing_public_key_bytes(0x30)).unwrap(),
            &recipient_identity(0x30),
        )
        .unwrap();
        let prepare_provider =
            FakeDecryptProvider::with_prepare_response(issuer, NOW, prepare_response);
        let prepared = prepare_recipient(&prepare_provider, &buy, audit_id, issuer, NOW, NOW + 50)
            .await
            .unwrap();

        let wrong = FakeDecryptProvider::with_cancel_response(
            DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                runtime_release_audit_id(0x72),
                *prepared.prepared_recipient_handle(),
            )
            .unwrap(),
        );
        assert_eq!(
            cancel_prepared_recipient(&wrong, &prepared).await,
            Err(RuntimeOpenError::DecryptResult)
        );
    }

    #[tokio::test]
    async fn close_viewer_session_accepts_exact_replay_and_already_absent() {
        let session = viewer_session(0x41);
        let audit_id = RuntimeReleaseAuditIdV1::new(session.audit_request_id()).unwrap();
        let closed = FakeDecryptProvider::with_close_response(
            DecryptProviderResponseV1::new_closed_viewer_session(
                audit_id,
                *session.viewer_session_handle(),
            )
            .unwrap(),
        );
        close_viewer_session(&closed, &session).await.unwrap();
        close_viewer_session(&closed, &session).await.unwrap();
        assert_eq!(closed.close_requests.lock().unwrap().len(), 2);

        let absent = FakeDecryptProvider::with_close_response(
            DecryptProviderResponseV1::new_viewer_session_already_absent(
                audit_id,
                *session.viewer_session_handle(),
            )
            .unwrap(),
        );
        close_viewer_session(&absent, &session).await.unwrap();
    }

    #[tokio::test]
    async fn close_viewer_session_rejects_conflicting_response_binding() {
        let session = viewer_session(0x41);
        let wrong = FakeDecryptProvider::with_close_response(
            DecryptProviderResponseV1::new_closed_viewer_session(
                runtime_release_audit_id(0x51),
                *session.viewer_session_handle(),
            )
            .unwrap(),
        );
        assert_eq!(
            close_viewer_session(&wrong, &session).await,
            Err(RuntimeOpenError::DecryptResult)
        );
    }
}
