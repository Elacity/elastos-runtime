use serde::Serialize;

use crate::canonical::{validate_ascii_identifier, CanonicalBody, ContractError, Decoder, Encoder};
use crate::{
    CanonicalContract, Digest32, ProtectedContentBindingV1, RightsActionV1, RightsPolicyIdentityV1,
    WalletAddress,
};

const MAX_POLICY_IDENTIFIER_BYTES: usize = 256;
const MAX_EVM_RIGHT_ARGUMENT_BYTES: usize = "download".len();

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
    HasAccessByContentIdStringAddressString = 1,
}

impl EvmRightsMethodAbiV1 {
    fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            1 => Ok(Self::HasAccessByContentIdStringAddressString),
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
pub struct RightsObservationFinalityV1 {
    min_confirmations: u16,
}

impl RightsObservationFinalityV1 {
    pub const fn new(min_confirmations: u16) -> Self {
        Self { min_confirmations }
    }

    pub const fn min_confirmations(&self) -> u16 {
        self.min_confirmations
    }
}

impl CanonicalBody for RightsObservationFinalityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-observation-finality/v1";

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.u16(self.min_confirmations);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Ok(Self::new(decoder.u16()?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsPolicyBodyV1 {
    content_id: String,
    required_action: RightsActionV1,
    evm_right_argument: String,
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
        content_id: impl Into<String>,
        required_action: RightsActionV1,
        evm_right_argument: impl Into<String>,
        subject_source: RightsSubjectSourceV1,
        chain_id: u64,
        contract_address: EvmContractAddressV1,
        function_selector: EvmFunctionSelectorV1,
        method_abi: EvmRightsMethodAbiV1,
        observation_finality: RightsObservationFinalityV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            content_id: content_id.into(),
            required_action,
            evm_right_argument: evm_right_argument.into(),
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

    pub fn content_id(&self) -> &str {
        &self.content_id
    }

    pub const fn required_action(&self) -> RightsActionV1 {
        self.required_action
    }

    pub fn evm_right_argument(&self) -> &str {
        &self.evm_right_argument
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
        validate_policy_identifier(&self.content_id, "content_id")?;
        validate_evm_right_argument(&self.evm_right_argument)?;
        if self.chain_id == 0 {
            return Err(ContractError::InvalidField("chain_id"));
        }
        EvmContractAddressV1::new(*self.contract_address.as_bytes())?;
        EvmFunctionSelectorV1::new(*self.function_selector.as_bytes())?;
        self.observation_finality.canonical_bytes()?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.string(&self.content_id)?;
        encoder.u8(self.required_action as u8);
        encoder.string(&self.evm_right_argument)?;
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
            decoder.string("content_id", MAX_POLICY_IDENTIFIER_BYTES)?,
            RightsActionV1::decode(decoder.u8()?)?,
            decoder.string("evm_right_argument", MAX_EVM_RIGHT_ARGUMENT_BYTES)?,
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
    binding: ProtectedContentBindingV1,
    policy_identity: RightsPolicyIdentityV1,
    subject_wallet: WalletAddress,
    observed_chain_id: u64,
    observed_block_number: u64,
    observed_block_hash: Digest32,
    head_block_number: u64,
    has_access: bool,
}

impl RightsEvaluationEvidenceV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding: ProtectedContentBindingV1,
        policy_identity: RightsPolicyIdentityV1,
        subject_wallet: WalletAddress,
        observed_chain_id: u64,
        observed_block_number: u64,
        observed_block_hash: Digest32,
        head_block_number: u64,
        has_access: bool,
    ) -> Result<Self, ContractError> {
        let value = Self {
            binding,
            policy_identity,
            subject_wallet,
            observed_chain_id,
            observed_block_number,
            observed_block_hash,
            head_block_number,
            has_access,
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

    pub const fn subject_wallet(&self) -> WalletAddress {
        self.subject_wallet
    }

    pub const fn observed_chain_id(&self) -> u64 {
        self.observed_chain_id
    }

    pub const fn observed_block_number(&self) -> u64 {
        self.observed_block_number
    }

    pub const fn observed_block_hash(&self) -> Digest32 {
        self.observed_block_hash
    }

    pub const fn head_block_number(&self) -> u64 {
        self.head_block_number
    }

    pub const fn has_access(&self) -> bool {
        self.has_access
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
        let required_confirmations = u64::from(policy.observation_finality().min_confirmations());
        if self
            .head_block_number
            .saturating_sub(self.observed_block_number)
            < required_confirmations
        {
            return Err(ContractError::InvalidField("evidence.head_block_number"));
        }
        Ok(())
    }
}

impl CanonicalBody for RightsEvaluationEvidenceV1 {
    const DOMAIN: &'static str = "elastos.protected-content.rights-evaluation-evidence/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.binding.canonical_bytes()?;
        self.policy_identity.canonical_bytes()?;
        if self.binding.rights_policy() != &self.policy_identity {
            return Err(ContractError::InvalidField("evidence.binding"));
        }
        if self.subject_wallet != self.binding.wallet() {
            return Err(ContractError::InvalidField("evidence.subject_wallet"));
        }
        if self.observed_chain_id == 0 {
            return Err(ContractError::InvalidField("evidence.observed_chain_id"));
        }
        if self.observed_block_hash == Digest32::new([0; 32]) {
            return Err(ContractError::InvalidField("evidence.observed_block_hash"));
        }
        if self.head_block_number < self.observed_block_number {
            return Err(ContractError::InvalidField("evidence.head_block_number"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.binding)?;
        encoder.nested(&self.policy_identity)?;
        encoder.fixed(self.subject_wallet.as_bytes());
        encoder.u64(self.observed_chain_id);
        encoder.u64(self.observed_block_number);
        encoder.fixed(self.observed_block_hash.as_bytes());
        encoder.u64(self.head_block_number);
        encoder.u8(u8::from(self.has_access));
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("binding")?,
            decoder.nested("policy_identity")?,
            WalletAddress::new(decoder.fixed()?),
            decoder.u64()?,
            decoder.u64()?,
            Digest32::new(decoder.fixed()?),
            decoder.u64()?,
            decode_bool(decoder, "has_access")?,
        )
    }
}

fn validate_policy_identifier(value: &str, field: &'static str) -> Result<(), ContractError> {
    validate_ascii_identifier(value, field, MAX_POLICY_IDENTIFIER_BYTES)?;
    if value.bytes().any(|byte| matches!(byte, b'/' | b'\\')) {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

fn validate_evm_right_argument(value: &str) -> Result<(), ContractError> {
    match value {
        "view" | "stream" | "download" | "execute" => Ok(()),
        _ => Err(ContractError::InvalidField("evm_right_argument")),
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

    fn policy() -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            "content:alpha",
            RightsActionV1::Stream,
            "stream",
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(12),
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

        assert_eq!(
            identity,
            RightsPolicyIdentityV1::new(policy.canonical_hash().unwrap(), 167).unwrap()
        );
        assert_eq!(
            encode(policy.canonical_hash().unwrap().as_bytes()),
            "f9e0147f85ca56d34c2325d5c559d1e464dd4b62e6dc32e46c7531f14438cb67"
        );
    }

    #[test]
    fn policy_rejects_invalid_contract_chain_and_selector() {
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
                "content:alpha",
                RightsActionV1::Stream,
                "raw_call",
                RightsSubjectSourceV1::WalletAddress,
                11155111,
                EvmContractAddressV1::new([0x11; 20]).unwrap(),
                EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
                EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
                RightsObservationFinalityV1::new(0),
            ),
            Err(ContractError::InvalidField("evm_right_argument"))
        );
        assert_eq!(
            RightsPolicyBodyV1::new(
                "content:alpha",
                RightsActionV1::Stream,
                "stream",
                RightsSubjectSourceV1::WalletAddress,
                0,
                EvmContractAddressV1::new([0x11; 20]).unwrap(),
                EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
                EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
                RightsObservationFinalityV1::new(0),
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
            binding.clone(),
            policy.policy_identity().unwrap(),
            binding.wallet(),
            policy.chain_id(),
            100,
            digest(0x55),
            112,
            true,
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
    fn evidence_rejects_wrong_wallet_chain_and_finality() {
        let policy = policy();
        let binding = policy_binding(&policy);
        let request = RightsEvaluationEvidenceRequestV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
        )
        .unwrap();
        let wrong_wallet = RightsEvaluationEvidenceV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
            wallet(8),
            policy.chain_id(),
            100,
            digest(0x55),
            112,
            true,
        )
        .unwrap_err();
        assert_eq!(
            wrong_wallet,
            ContractError::InvalidField("evidence.subject_wallet")
        );

        let wrong_chain = RightsEvaluationEvidenceV1::new(
            binding.clone(),
            policy.policy_identity().unwrap(),
            binding.wallet(),
            1,
            100,
            digest(0x55),
            112,
            true,
        )
        .unwrap();
        assert_eq!(
            wrong_chain.validate_against_request(&request, &policy),
            Err(ContractError::InvalidField("evidence.observed_chain_id"))
        );

        let low_finality = RightsEvaluationEvidenceV1::new(
            binding,
            policy.policy_identity().unwrap(),
            wallet(7),
            policy.chain_id(),
            100,
            digest(0x55),
            111,
            true,
        )
        .unwrap();
        assert_eq!(
            low_finality.validate_against_request(&request, &policy),
            Err(ContractError::InvalidField("evidence.head_block_number"))
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
    fn policy_identity_changes_when_exact_contract_right_argument_changes() {
        let stream = policy();
        let view = RightsPolicyBodyV1::new(
            "content:alpha",
            RightsActionV1::Stream,
            "view",
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
            RightsObservationFinalityV1::new(12),
        )
        .unwrap();

        assert_ne!(
            stream.policy_identity().unwrap(),
            view.policy_identity().unwrap()
        );
    }
}
