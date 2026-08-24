use serde::Serialize;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::rights::{validate_active, validate_time_window};
use crate::{
    AuthenticatedRuntimeReleaseOperationV1, CanonicalContract, Digest32,
    EncryptedContentIdentityV1, ProtectedContentBindingV1, RightsActionV1, RightsPolicyIdentityV1,
    WalletAddress,
};

pub const MAX_RIGHTS_EVIDENCE_LIFETIME_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ContentAccessIdV1([u8; 16]);

impl ContentAccessIdV1 {
    pub fn new(bytes: [u8; 16]) -> Result<Self, ContractError> {
        if bytes == [0; 16] {
            return Err(ContractError::InvalidField("content_access_id"));
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl CanonicalBody for ContentAccessIdV1 {
    const DOMAIN: &'static str = "elastos.protected-content.content-access-id/v1";

    fn validate(&self) -> Result<(), ContractError> {
        ContentAccessIdV1::new(self.0)?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(decoder.fixed()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct EvmContractAddressV1([u8; 20]);

impl EvmContractAddressV1 {
    pub fn new(bytes: [u8; 20]) -> Result<Self, ContractError> {
        if bytes == [0; 20] {
            return Err(ContractError::InvalidField("evm_contract_address"));
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct EvmFunctionSelectorV1([u8; 4]);

impl EvmFunctionSelectorV1 {
    pub fn new(bytes: [u8; 4]) -> Result<Self, ContractError> {
        if bytes == [0; 4] {
            return Err(ContractError::InvalidField("evm_function_selector"));
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
pub enum EvmRightsMethodAbiV1 {
    HasAccessByContentIdAddressBytes16 = 1,
}

impl EvmRightsMethodAbiV1 {
    fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            1 => Ok(Self::HasAccessByContentIdAddressBytes16),
            _ => Err(ContractError::InvalidField("evm_rights_method_abi")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
pub enum RightsSubjectSourceV1 {
    WalletAddress = 1,
}

impl RightsSubjectSourceV1 {
    fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            1 => Ok(Self::WalletAddress),
            _ => Err(ContractError::InvalidField("rights_subject_source")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum RightsObservationFinalityV1 {
    Finalized = 1,
}

impl RightsObservationFinalityV1 {
    pub const fn finalized() -> Self {
        Self::Finalized
    }

    fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            1 => Ok(Self::Finalized),
            _ => Err(ContractError::InvalidField("rights_observation_finality")),
        }
    }
}

impl CanonicalBody for RightsObservationFinalityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-observation-finality/v1";

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.u8(*self as u8);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::decode(decoder.u8()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsPolicyBodyV1 {
    encrypted_content: EncryptedContentIdentityV1,
    content_access_id: ContentAccessIdV1,
    required_action: RightsActionV1,
    subject_source: RightsSubjectSourceV1,
    chain_id: u64,
    contract_address: EvmContractAddressV1,
    function_selector: EvmFunctionSelectorV1,
    method_abi: EvmRightsMethodAbiV1,
    observation_finality: RightsObservationFinalityV1,
}

impl RightsPolicyBodyV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        encrypted_content: EncryptedContentIdentityV1,
        content_access_id: ContentAccessIdV1,
        required_action: RightsActionV1,
        subject_source: RightsSubjectSourceV1,
        chain_id: u64,
        contract_address: EvmContractAddressV1,
        function_selector: EvmFunctionSelectorV1,
        method_abi: EvmRightsMethodAbiV1,
        observation_finality: RightsObservationFinalityV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            encrypted_content,
            content_access_id,
            required_action,
            subject_source,
            chain_id,
            contract_address,
            function_selector,
            method_abi,
            observation_finality,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub const fn content_access_id(&self) -> ContentAccessIdV1 {
        self.content_access_id
    }

    pub const fn required_action(&self) -> RightsActionV1 {
        self.required_action
    }

    pub const fn subject_source(&self) -> RightsSubjectSourceV1 {
        self.subject_source
    }

    pub const fn chain_id(&self) -> u64 {
        self.chain_id
    }

    pub const fn contract_address(&self) -> EvmContractAddressV1 {
        self.contract_address
    }

    pub const fn function_selector(&self) -> EvmFunctionSelectorV1 {
        self.function_selector
    }

    pub const fn method_abi(&self) -> EvmRightsMethodAbiV1 {
        self.method_abi
    }

    pub const fn observation_finality(&self) -> RightsObservationFinalityV1 {
        self.observation_finality
    }

    pub fn policy_identity(&self) -> Result<RightsPolicyIdentityV1, ContractError> {
        RightsPolicyIdentityV1::new(
            self.canonical_hash()?,
            u32::try_from(self.canonical_bytes()?.len())
                .map_err(|_| ContractError::InvalidField("policy_bytes"))?,
        )
    }
}

impl CanonicalBody for RightsPolicyBodyV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-policy-body/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.encrypted_content.canonical_bytes()?;
        ContentAccessIdV1::new(*self.content_access_id.as_bytes())?;
        if self.chain_id == 0 {
            return Err(ContractError::InvalidField("chain_id"));
        }
        EvmContractAddressV1::new(*self.contract_address.as_bytes())?;
        EvmFunctionSelectorV1::new(*self.function_selector.as_bytes())?;
        self.observation_finality.canonical_bytes()?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.encrypted_content)?;
        encoder.fixed(self.content_access_id.as_bytes());
        encoder.u8(self.required_action as u8);
        encoder.u8(self.subject_source as u8);
        encoder.u64(self.chain_id);
        encoder.fixed(self.contract_address.as_bytes());
        encoder.fixed(self.function_selector.as_bytes());
        encoder.u8(self.method_abi as u8);
        encoder.nested(&self.observation_finality)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("encrypted_content")?,
            ContentAccessIdV1::new(decoder.fixed()?)?,
            RightsActionV1::decode(decoder.u8()?)?,
            RightsSubjectSourceV1::decode(decoder.u8()?)?,
            decoder.u64()?,
            EvmContractAddressV1::new(decoder.fixed()?)?,
            EvmFunctionSelectorV1::new(decoder.fixed()?)?,
            EvmRightsMethodAbiV1::decode(decoder.u8()?)?,
            decoder.nested("observation_finality")?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsEvaluationEvidenceRequestV1 {
    binding: ProtectedContentBindingV1,
    policy_identity: RightsPolicyIdentityV1,
}

impl RightsEvaluationEvidenceRequestV1 {
    pub fn new(
        binding: ProtectedContentBindingV1,
        policy_identity: RightsPolicyIdentityV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            binding,
            policy_identity,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn policy_identity(&self) -> &RightsPolicyIdentityV1 {
        &self.policy_identity
    }

    pub fn validate_against_policy(
        &self,
        policy: &RightsPolicyBodyV1,
    ) -> Result<(), ContractError> {
        let policy_identity = policy.policy_identity()?;
        if self.policy_identity != policy_identity {
            return Err(ContractError::InvalidField(
                "evidence_request.policy_identity",
            ));
        }
        if self.binding.rights_policy() != &self.policy_identity {
            return Err(ContractError::InvalidField("evidence_request.binding"));
        }
        Ok(())
    }
}

impl CanonicalBody for RightsEvaluationEvidenceRequestV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-evaluation-evidence-request/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.policy_identity.canonical_bytes()?;
        if self.binding.rights_policy() != &self.policy_identity {
            return Err(ContractError::InvalidField("evidence_request.binding"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.nested(&self.policy_identity)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("binding")?,
            decoder.nested("policy_identity")?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsEvaluationEvidenceV1 {
    runtime_operation_hash: Digest32,
    release_request_hash: Digest32,
    binding: ProtectedContentBindingV1,
    policy_identity: RightsPolicyIdentityV1,
    subject_wallet: WalletAddress,
    observed_chain_id: u64,
    finalized_block_number: u64,
    finalized_block_hash: Digest32,
    has_access: bool,
    acquired_at: u64,
    expires_at: u64,
}

impl RightsEvaluationEvidenceV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runtime_operation_hash: Digest32,
        release_request_hash: Digest32,
        binding: ProtectedContentBindingV1,
        policy_identity: RightsPolicyIdentityV1,
        subject_wallet: WalletAddress,
        observed_chain_id: u64,
        finalized_block_number: u64,
        finalized_block_hash: Digest32,
        has_access: bool,
        acquired_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            runtime_operation_hash,
            release_request_hash,
            binding,
            policy_identity,
            subject_wallet,
            observed_chain_id,
            finalized_block_number,
            finalized_block_hash,
            has_access,
            acquired_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn runtime_operation_hash(&self) -> Digest32 {
        self.runtime_operation_hash
    }

    pub const fn release_request_hash(&self) -> Digest32 {
        self.release_request_hash
    }

    pub fn binding(&self) -> &ProtectedContentBindingV1 {
        &self.binding
    }

    pub const fn policy_identity(&self) -> &RightsPolicyIdentityV1 {
        &self.policy_identity
    }

    pub const fn subject_wallet(&self) -> WalletAddress {
        self.subject_wallet
    }

    pub const fn observed_chain_id(&self) -> u64 {
        self.observed_chain_id
    }

    pub const fn finalized_block_number(&self) -> u64 {
        self.finalized_block_number
    }

    pub const fn finalized_block_hash(&self) -> Digest32 {
        self.finalized_block_hash
    }

    pub const fn has_access(&self) -> bool {
        self.has_access
    }

    pub const fn acquired_at(&self) -> u64 {
        self.acquired_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn validate_against_request(
        &self,
        request: &RightsEvaluationEvidenceRequestV1,
        policy: &RightsPolicyBodyV1,
    ) -> Result<(), ContractError> {
        request.validate_against_policy(policy)?;
        if self.binding != *request.binding() {
            return Err(ContractError::InvalidField("evidence.binding"));
        }
        if self.policy_identity != *request.policy_identity() {
            return Err(ContractError::InvalidField("evidence.policy_identity"));
        }
        if self.subject_wallet != self.binding.wallet() {
            return Err(ContractError::InvalidField("evidence.subject_wallet"));
        }
        if self.observed_chain_id != policy.chain_id() {
            return Err(ContractError::InvalidField("evidence.observed_chain_id"));
        }
        Ok(())
    }

    pub fn validate_against_runtime_release_at(
        &self,
        operation: &AuthenticatedRuntimeReleaseOperationV1,
        now: u64,
    ) -> Result<(), ContractError> {
        self.validate_against_request(
            operation.statement().evidence_request(),
            operation.statement().policy_body(),
        )?;
        if self.runtime_operation_hash != operation.operation_hash() {
            return Err(ContractError::InvalidField(
                "evidence.runtime_operation_hash",
            ));
        }
        if self.release_request_hash != operation.release_request_hash() {
            return Err(ContractError::InvalidField("evidence.release_request_hash"));
        }
        if self.acquired_at < operation.statement().issued_at()
            || self.expires_at > operation.statement().expires_at()
        {
            return Err(ContractError::InvalidField("evidence.window"));
        }
        validate_active(self.acquired_at, self.expires_at, now)
            .map_err(|_| ContractError::InvalidField("evidence.window"))
    }
}

impl CanonicalBody for RightsEvaluationEvidenceV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-evaluation-evidence/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.policy_identity.canonical_bytes()?;
        validate_time_window(
            self.acquired_at,
            self.expires_at,
            MAX_RIGHTS_EVIDENCE_LIFETIME_SECS,
            "rights_evaluation_evidence_lifetime",
        )?;
        if self.binding.rights_policy() != &self.policy_identity {
            return Err(ContractError::InvalidField("evidence.binding"));
        }
        if self.subject_wallet != self.binding.wallet() {
            return Err(ContractError::InvalidField("evidence.subject_wallet"));
        }
        if self.observed_chain_id == 0 {
            return Err(ContractError::InvalidField("evidence.observed_chain_id"));
        }
        if self.finalized_block_hash == Digest32::new([0; 32]) {
            return Err(ContractError::InvalidField("evidence.finalized_block_hash"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.runtime_operation_hash.as_bytes());
        encoder.fixed(self.release_request_hash.as_bytes());
        encoder.nested(&self.binding)?;
        encoder.nested(&self.policy_identity)?;
        encoder.fixed(self.subject_wallet.as_bytes());
        encoder.u64(self.observed_chain_id);
        encoder.u64(self.finalized_block_number);
        encoder.fixed(self.finalized_block_hash.as_bytes());
        encoder.u8(u8::from(self.has_access));
        encoder.u64(self.acquired_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            Digest32::new(decoder.fixed()?),
            Digest32::new(decoder.fixed()?),
            decoder.nested("binding")?,
            decoder.nested("policy_identity")?,
            WalletAddress::new(decoder.fixed()?),
            decoder.u64()?,
            decoder.u64()?,
            Digest32::new(decoder.fixed()?),
            decode_bool(decoder, "has_access")?,
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

fn decode_bool(decoder: &mut Decoder<'_>, field: &'static str) -> Result<bool, ContractError> {
    match decoder.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ContractError::InvalidField(field)),
    }
}

#[cfg(test)]
mod tests {
    use hex::encode;

    use super::*;
    use crate::test_support::{binding_for_wallet, digest, wallet};

    fn encrypted_content(seed: u8) -> EncryptedContentIdentityV1 {
        EncryptedContentIdentityV1::new(digest(seed), 2048).unwrap()
    }

    fn access_id(seed: u8) -> ContentAccessIdV1 {
        ContentAccessIdV1::new([seed; 16]).unwrap()
    }

    fn policy() -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            encrypted_content(0x10),
            access_id(0x20),
            RightsActionV1::Stream,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap()
    }

    fn policy_binding(policy: &RightsPolicyBodyV1) -> ProtectedContentBindingV1 {
        let base = binding_for_wallet(wallet(7));
        ProtectedContentBindingV1::new(
            base.encrypted_content().clone(),
            base.key_envelope().clone(),
            policy.policy_identity().unwrap(),
            base.profile(),
            base.wallet(),
            base.runtime_session_binding(),
        )
        .unwrap()
    }

    #[test]
    fn policy_identity_matches_exact_policy_bytes() {
        let policy = policy();
        let identity = policy.policy_identity().unwrap();
        let canonical = policy.canonical_bytes().unwrap();
        const EXPECTED_CANONICAL_LEN: usize = 248;
        const EXPECTED_POLICY_SHA256: &str =
            "2921fd94863d6f21c378ec930ac6f5f37a4407373411a1139800fcbf6ed300df";

        assert_eq!(canonical.len(), EXPECTED_CANONICAL_LEN);
        assert_eq!(
            encode(policy.canonical_hash().unwrap().as_bytes()),
            EXPECTED_POLICY_SHA256
        );
        assert_eq!(
            identity,
            RightsPolicyIdentityV1::new(
                policy.canonical_hash().unwrap(),
                u32::try_from(EXPECTED_CANONICAL_LEN).unwrap(),
            )
            .unwrap()
        );
        assert_eq!(
            encode(identity.policy_sha256().as_bytes()),
            EXPECTED_POLICY_SHA256
        );
    }

    #[test]
    fn policy_rejects_invalid_access_id_contract_chain_and_selector() {
        assert_eq!(
            ContentAccessIdV1::new([0; 16]),
            Err(ContractError::InvalidField("content_access_id"))
        );
        assert_eq!(
            EvmContractAddressV1::new([0; 20]),
            Err(ContractError::InvalidField("evm_contract_address"))
        );
        assert_eq!(
            EvmFunctionSelectorV1::new([0; 4]),
            Err(ContractError::InvalidField("evm_function_selector"))
        );
        assert_eq!(
            RightsPolicyBodyV1::new(
                encrypted_content(0x10),
                access_id(0x20),
                RightsActionV1::Stream,
                RightsSubjectSourceV1::WalletAddress,
                0,
                EvmContractAddressV1::new([0x11; 20]).unwrap(),
                EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
                EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
                RightsObservationFinalityV1::finalized(),
            ),
            Err(ContractError::InvalidField("chain_id"))
        );
    }

    #[test]
    fn evidence_request_and_result_bind_exact_policy_wallet_and_observation() {
        let policy = policy();
        let binding = policy_binding(&policy);
        let request = RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
        )
        .unwrap();
        request.validate_against_policy(&policy).unwrap();

        let evidence = RightsEvaluationEvidenceV1::new(
            digest(0x90),
            digest(0x91),
            binding.clone(),
            policy.policy_identity().unwrap(),
            binding.wallet(),
            policy.chain_id(),
            100,
            digest(0x55),
            true,
            1_000,
            1_030,
        )
        .unwrap();
        evidence
            .validate_against_request(&request, &policy)
            .unwrap();

        let wrong_policy = RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            RightsPolicyIdentityV1::new(digest(0xee), 111).unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            wrong_policy,
            ContractError::InvalidField("evidence_request.binding")
        );
    }

    #[test]
    fn evidence_rejects_wrong_wallet_and_chain() {
        let policy = policy();
        let binding = policy_binding(&policy);
        let request = RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
        )
        .unwrap();
        let wrong_wallet = RightsEvaluationEvidenceV1::new(
            digest(0x90),
            digest(0x91),
            binding.clone(),
            policy.policy_identity().unwrap(),
            wallet(8),
            policy.chain_id(),
            100,
            digest(0x55),
            true,
            1_000,
            1_030,
        )
        .unwrap_err();
        assert_eq!(
            wrong_wallet,
            ContractError::InvalidField("evidence.subject_wallet")
        );

        let wrong_chain = RightsEvaluationEvidenceV1::new(
            digest(0x90),
            digest(0x91),
            binding.clone(),
            policy.policy_identity().unwrap(),
            binding.wallet(),
            1,
            100,
            digest(0x55),
            true,
            1_000,
            1_030,
        )
        .unwrap();
        assert_eq!(
            wrong_chain.validate_against_request(&request, &policy),
            Err(ContractError::InvalidField("evidence.observed_chain_id"))
        );
    }

    #[test]
    fn policy_decode_rejects_trailing_and_noncanonical_flags() {
        let policy = policy();
        let mut bytes = policy.canonical_bytes().unwrap();
        bytes.push(0);
        assert_eq!(
            RightsPolicyBodyV1::from_canonical_bytes(&bytes),
            Err(ContractError::TrailingBytes)
        );

        let mut noncanonical = policy.canonical_bytes().unwrap();
        let finality_bytes = policy.observation_finality().canonical_bytes().unwrap();
        let method_abi_index = noncanonical.len() - finality_bytes.len() - 2 - 1;
        noncanonical[method_abi_index] = 2;
        assert_eq!(
            RightsPolicyBodyV1::from_canonical_bytes(&noncanonical),
            Err(ContractError::InvalidField("evm_rights_method_abi"))
        );
    }

    #[test]
    fn policy_identity_changes_when_exact_content_or_access_id_changes() {
        let base = policy();
        let changed_content = RightsPolicyBodyV1::new(
            encrypted_content(0x11),
            access_id(0x20),
            RightsActionV1::Stream,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap();
        let changed_access = RightsPolicyBodyV1::new(
            encrypted_content(0x10),
            access_id(0x21),
            RightsActionV1::Stream,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap();

        assert_ne!(
            base.policy_identity().unwrap(),
            changed_content.policy_identity().unwrap()
        );
        assert_ne!(
            base.policy_identity().unwrap(),
            changed_access.policy_identity().unwrap()
        );
    }
}
