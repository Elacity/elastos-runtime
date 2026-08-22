use std::collections::HashSet;

use ed25519_dalek::Signature;
use serde::Serialize;
use thiserror::Error;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::identity::validate_ed25519_public_key;
use crate::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyEpochError, CustodyEpochIdentityV1,
    CustodyEpochIssuerKeyV1, Digest32, NodeCustodyPublicKeyV1, NodePublicKey, SignedCustodyEpochV1,
    VerifiedCustodyEpochV1,
};

const ED25519_SIGNATURE_BYTES: usize = 64;
const MAX_CUSTODY_POOL_MEMBERS_V1: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CustodyPoolOperatorIdV1([u8; 32]);

impl CustodyPoolOperatorIdV1 {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl CanonicalBody for CustodyPoolOperatorIdV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-pool.operator-id/v1";

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Ok(Self::new(decoder.fixed()?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CustodyPoolFailureDomainIdV1([u8; 32]);

impl CustodyPoolFailureDomainIdV1 {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl CanonicalBody for CustodyPoolFailureDomainIdV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-pool.failure-domain-id/v1";

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.as_bytes());
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Ok(Self::new(decoder.fixed()?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
pub enum CustodyPoolMemberStateV1 {
    Active = 0,
    Revoked = 1,
}

impl CustodyPoolMemberStateV1 {
    fn decode(value: u8) -> Result<Self, ContractError> {
        match value {
            0 => Ok(Self::Active),
            1 => Ok(Self::Revoked),
            _ => Err(ContractError::InvalidField("custody_pool_member_state")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyPoolMemberV1 {
    node_public_key: NodePublicKey,
    custody_public_key: NodeCustodyPublicKeyV1,
    operator_id: CustodyPoolOperatorIdV1,
    failure_domain_id: CustodyPoolFailureDomainIdV1,
    approved_suites: CustodyApprovedSuitesV1,
    valid_from: u64,
    valid_until: u64,
    state: CustodyPoolMemberStateV1,
}

impl CustodyPoolMemberV1 {
    pub fn new(
        node_public_key: NodePublicKey,
        custody_public_key: NodeCustodyPublicKeyV1,
        operator_id: CustodyPoolOperatorIdV1,
        failure_domain_id: CustodyPoolFailureDomainIdV1,
        approved_suites: CustodyApprovedSuitesV1,
        active_window: (u64, u64),
        state: CustodyPoolMemberStateV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            node_public_key,
            custody_public_key,
            operator_id,
            failure_domain_id,
            approved_suites,
            valid_from: active_window.0,
            valid_until: active_window.1,
            state,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn node_public_key(&self) -> NodePublicKey {
        self.node_public_key
    }

    pub const fn custody_public_key(&self) -> NodeCustodyPublicKeyV1 {
        self.custody_public_key
    }

    pub const fn operator_id(&self) -> CustodyPoolOperatorIdV1 {
        self.operator_id
    }

    pub const fn failure_domain_id(&self) -> CustodyPoolFailureDomainIdV1 {
        self.failure_domain_id
    }

    pub fn approved_suites(&self) -> &CustodyApprovedSuitesV1 {
        &self.approved_suites
    }

    pub const fn valid_from(&self) -> u64 {
        self.valid_from
    }

    pub const fn valid_until(&self) -> u64 {
        self.valid_until
    }

    pub const fn state(&self) -> CustodyPoolMemberStateV1 {
        self.state
    }
}

impl CanonicalBody for CustodyPoolMemberV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-pool.member/v1";

    fn validate(&self) -> Result<(), ContractError> {
        NodePublicKey::new(*self.node_public_key.as_bytes())?;
        NodeCustodyPublicKeyV1::new(*self.custody_public_key.as_bytes())?;
        self.operator_id.canonical_bytes()?;
        self.failure_domain_id.canonical_bytes()?;
        self.approved_suites.canonical_bytes()?;
        if self.valid_from >= self.valid_until {
            return Err(ContractError::InvalidField("custody_pool_member_window"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.node_public_key.as_bytes());
        encoder.fixed(self.custody_public_key.as_bytes());
        encoder.nested(&self.operator_id)?;
        encoder.nested(&self.failure_domain_id)?;
        encoder.nested(&self.approved_suites)?;
        encoder.u64(self.valid_from);
        encoder.u64(self.valid_until);
        encoder.u8(self.state as u8);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            NodePublicKey::new(decoder.fixed()?)?,
            NodeCustodyPublicKeyV1::new(decoder.fixed()?)?,
            decoder.nested("operator_id")?,
            decoder.nested("failure_domain_id")?,
            decoder.nested("approved_suites")?,
            (decoder.u64()?, decoder.u64()?),
            CustodyPoolMemberStateV1::decode(decoder.u8()?)?,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct CustodyPoolIdentityV1 {
    pool_sha256: Digest32,
    pool_bytes: u32,
}

impl CustodyPoolIdentityV1 {
    pub fn new(pool_sha256: Digest32, pool_bytes: u32) -> Result<Self, ContractError> {
        let value = Self {
            pool_sha256,
            pool_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn pool_sha256(&self) -> Digest32 {
        self.pool_sha256
    }

    pub const fn pool_bytes(&self) -> u32 {
        self.pool_bytes
    }
}

impl CanonicalBody for CustodyPoolIdentityV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-pool.identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        if self.pool_bytes == 0 || self.pool_bytes > (u16::MAX as u32) {
            return Err(ContractError::InvalidField("pool_bytes"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.pool_sha256.as_bytes());
        encoder.u32(self.pool_bytes);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?), decoder.u32()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct CustodyCommitteeAuthorizationIdentityV1 {
    authorization_sha256: Digest32,
    authorization_bytes: u32,
}

impl CustodyCommitteeAuthorizationIdentityV1 {
    pub fn new(
        authorization_sha256: Digest32,
        authorization_bytes: u32,
    ) -> Result<Self, ContractError> {
        let value = Self {
            authorization_sha256,
            authorization_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn authorization_sha256(&self) -> Digest32 {
        self.authorization_sha256
    }

    pub const fn authorization_bytes(&self) -> u32 {
        self.authorization_bytes
    }
}

impl CanonicalBody for CustodyCommitteeAuthorizationIdentityV1 {
    const DOMAIN: &'static str =
        "elastos.protected-content.custody-committee-authorization.identity/v1";

    fn validate(&self) -> Result<(), ContractError> {
        if self.authorization_bytes == 0 || self.authorization_bytes > (u16::MAX as u32) {
            return Err(ContractError::InvalidField("authorization_bytes"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.authorization_sha256.as_bytes());
        encoder.u32(self.authorization_bytes);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(Digest32::new(decoder.fixed()?), decoder.u32()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyPoolStatementV1 {
    issuer: CustodyEpochIssuerKeyV1,
    members: Vec<CustodyPoolMemberV1>,
}

impl CustodyPoolStatementV1 {
    pub fn new(
        issuer: CustodyEpochIssuerKeyV1,
        mut members: Vec<CustodyPoolMemberV1>,
    ) -> Result<Self, ContractError> {
        members.sort_unstable_by_key(|member| member.node_public_key());
        let value = Self { issuer, members };
        value.validate()?;
        Ok(value)
    }

    pub const fn issuer(&self) -> CustodyEpochIssuerKeyV1 {
        self.issuer
    }

    pub fn members(&self) -> &[CustodyPoolMemberV1] {
        &self.members
    }
}

impl CanonicalBody for CustodyPoolStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.custody-pool.statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        CustodyEpochIssuerKeyV1::new(*self.issuer.as_bytes())?;
        if self.members.is_empty() || self.members.len() > MAX_CUSTODY_POOL_MEMBERS_V1 {
            return Err(ContractError::InvalidField("custody_pool_members"));
        }
        for (index, member) in self.members.iter().enumerate() {
            member.canonical_bytes()?;
            if index > 0 && self.members[index - 1].node_public_key() >= member.node_public_key() {
                return Err(ContractError::InvalidField("custody_pool_members"));
            }
        }
        let mut custody_keys = HashSet::with_capacity(self.members.len());
        for member in &self.members {
            if !custody_keys.insert(member.custody_public_key()) {
                return Err(ContractError::InvalidField("node_custody_public_key"));
            }
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.issuer.as_bytes());
        encoder.u8(u8::try_from(self.members.len())
            .map_err(|_| ContractError::InvalidField("custody_pool_members"))?);
        for member in &self.members {
            encoder.nested(member)?;
        }
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        let issuer = CustodyEpochIssuerKeyV1::new(decoder.fixed()?)?;
        let count = usize::from(decoder.u8()?);
        if count == 0 || count > MAX_CUSTODY_POOL_MEMBERS_V1 {
            return Err(ContractError::InvalidField("custody_pool_members"));
        }
        let mut members = Vec::with_capacity(count);
        for _ in 0..count {
            members.push(decoder.nested("custody_pool_member")?);
        }
        Self::new(issuer, members)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCustodyPoolV1 {
    statement: CustodyPoolStatementV1,
    issuer_signature: Vec<u8>,
}

impl SignedCustodyPoolV1 {
    pub fn new(
        statement: CustodyPoolStatementV1,
        issuer_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            issuer_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &CustodyPoolStatementV1 {
        &self.statement
    }

    pub fn pool_identity(&self) -> Result<CustodyPoolIdentityV1, ContractError> {
        CustodyPoolIdentityV1::new(
            self.canonical_hash()?,
            u32::try_from(self.canonical_bytes()?.len())
                .map_err(|_| ContractError::InvalidField("pool_bytes"))?,
        )
    }

    pub fn verify(
        &self,
        expected_issuer: CustodyEpochIssuerKeyV1,
    ) -> Result<VerifiedCustodyPoolV1, CustodyPoolError> {
        self.canonical_bytes()?;
        if self.statement.issuer != expected_issuer {
            return Err(CustodyPoolError::UnexpectedIssuer("custody_pool_issuer"));
        }
        let key =
            validate_ed25519_public_key(*self.statement.issuer.as_bytes(), "custody_pool_issuer")
                .map_err(|_| CustodyPoolError::InvalidPoolSignature)?;
        let signature = Signature::from_slice(&self.issuer_signature)
            .map_err(|_| CustodyPoolError::InvalidPoolSignature)?;
        key.verify_strict(&self.statement.canonical_bytes()?, &signature)
            .map_err(|_| CustodyPoolError::InvalidPoolSignature)?;
        Ok(VerifiedCustodyPoolV1 {
            pool_identity: self.pool_identity()?,
            issuer: self.statement.issuer,
            members: self.statement.members.clone(),
        })
    }
}

impl CanonicalBody for SignedCustodyPoolV1 {
    const DOMAIN: &'static str = "elastos.protected-content.signed-custody-pool/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.issuer_signature.len() != ED25519_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("custody_pool_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.issuer_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("statement")?,
            decoder.bytes("issuer_signature", ED25519_SIGNATURE_BYTES)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCustodyPoolV1 {
    pool_identity: CustodyPoolIdentityV1,
    issuer: CustodyEpochIssuerKeyV1,
    members: Vec<CustodyPoolMemberV1>,
}

impl VerifiedCustodyPoolV1 {
    pub const fn pool_identity(&self) -> CustodyPoolIdentityV1 {
        self.pool_identity
    }

    pub const fn issuer(&self) -> CustodyEpochIssuerKeyV1 {
        self.issuer
    }

    pub fn members(&self) -> &[CustodyPoolMemberV1] {
        &self.members
    }

    pub fn member(&self, node_public_key: NodePublicKey) -> Option<&CustodyPoolMemberV1> {
        self.members
            .binary_search_by_key(&node_public_key, |member| member.node_public_key())
            .ok()
            .map(|index| &self.members[index])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyCommitteeAuthorizationStatementV1 {
    issuer: CustodyEpochIssuerKeyV1,
    pool_identity: CustodyPoolIdentityV1,
    custody_epoch_identity: CustodyEpochIdentityV1,
}

impl CustodyCommitteeAuthorizationStatementV1 {
    pub fn new(
        issuer: CustodyEpochIssuerKeyV1,
        pool_identity: CustodyPoolIdentityV1,
        custody_epoch_identity: CustodyEpochIdentityV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            issuer,
            pool_identity,
            custody_epoch_identity,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn issuer(&self) -> CustodyEpochIssuerKeyV1 {
        self.issuer
    }

    pub const fn pool_identity(&self) -> CustodyPoolIdentityV1 {
        self.pool_identity
    }

    pub const fn custody_epoch_identity(&self) -> CustodyEpochIdentityV1 {
        self.custody_epoch_identity
    }
}

impl CanonicalBody for CustodyCommitteeAuthorizationStatementV1 {
    const DOMAIN: &'static str =
        "elastos.protected-content.custody-committee-authorization-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        CustodyEpochIssuerKeyV1::new(*self.issuer.as_bytes())?;
        self.pool_identity.canonical_bytes()?;
        self.custody_epoch_identity.canonical_bytes()?;
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.issuer.as_bytes());
        encoder.nested(&self.pool_identity)?;
        encoder.nested(&self.custody_epoch_identity)?;
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            CustodyEpochIssuerKeyV1::new(decoder.fixed()?)?,
            decoder.nested("pool_identity")?,
            decoder.nested("custody_epoch_identity")?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCustodyCommitteeAuthorizationV1 {
    statement: CustodyCommitteeAuthorizationStatementV1,
    issuer_signature: Vec<u8>,
}

impl SignedCustodyCommitteeAuthorizationV1 {
    pub fn new(
        statement: CustodyCommitteeAuthorizationStatementV1,
        issuer_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            issuer_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &CustodyCommitteeAuthorizationStatementV1 {
        &self.statement
    }

    pub fn authorization_identity(
        &self,
    ) -> Result<CustodyCommitteeAuthorizationIdentityV1, ContractError> {
        CustodyCommitteeAuthorizationIdentityV1::new(
            self.canonical_hash()?,
            u32::try_from(self.canonical_bytes()?.len())
                .map_err(|_| ContractError::InvalidField("authorization_bytes"))?,
        )
    }

    pub fn verify(
        &self,
        expected_issuer: CustodyEpochIssuerKeyV1,
        expected_authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
        expected_pool_identity: CustodyPoolIdentityV1,
        expected_epoch_identity: CustodyEpochIdentityV1,
    ) -> Result<VerifiedCustodyCommitteeAuthorizationV1, CustodyPoolError> {
        self.canonical_bytes()?;
        let authorization_identity = self.authorization_identity()?;
        if authorization_identity != expected_authorization_identity {
            return Err(CustodyPoolError::BindingMismatch(
                "custody_committee_authorization_identity",
            ));
        }
        if self.statement.issuer != expected_issuer {
            return Err(CustodyPoolError::UnexpectedIssuer(
                "custody_committee_authorization_issuer",
            ));
        }
        if self.statement.pool_identity != expected_pool_identity {
            return Err(CustodyPoolError::BindingMismatch("custody_pool_identity"));
        }
        if self.statement.custody_epoch_identity != expected_epoch_identity {
            return Err(CustodyPoolError::BindingMismatch("custody_epoch_identity"));
        }
        let key = validate_ed25519_public_key(
            *self.statement.issuer.as_bytes(),
            "custody_committee_authorization_issuer",
        )
        .map_err(|_| CustodyPoolError::InvalidCommitteeAuthorizationSignature)?;
        let signature = Signature::from_slice(&self.issuer_signature)
            .map_err(|_| CustodyPoolError::InvalidCommitteeAuthorizationSignature)?;
        key.verify_strict(&self.statement.canonical_bytes()?, &signature)
            .map_err(|_| CustodyPoolError::InvalidCommitteeAuthorizationSignature)?;
        Ok(VerifiedCustodyCommitteeAuthorizationV1 {
            authorization_identity,
            issuer: self.statement.issuer,
            pool_identity: self.statement.pool_identity,
            custody_epoch_identity: self.statement.custody_epoch_identity,
        })
    }
}

impl CanonicalBody for SignedCustodyCommitteeAuthorizationV1 {
    const DOMAIN: &'static str =
        "elastos.protected-content.signed-custody-committee-authorization/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.issuer_signature.len() != ED25519_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField(
                "custody_committee_authorization_signature",
            ));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.issuer_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("statement")?,
            decoder.bytes(
                "custody_committee_authorization_signature",
                ED25519_SIGNATURE_BYTES,
            )?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCustodyCommitteeAuthorizationV1 {
    authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
    issuer: CustodyEpochIssuerKeyV1,
    pool_identity: CustodyPoolIdentityV1,
    custody_epoch_identity: CustodyEpochIdentityV1,
}

impl VerifiedCustodyCommitteeAuthorizationV1 {
    pub const fn authorization_identity(&self) -> CustodyCommitteeAuthorizationIdentityV1 {
        self.authorization_identity
    }

    pub const fn issuer(&self) -> CustodyEpochIssuerKeyV1 {
        self.issuer
    }

    pub const fn pool_identity(&self) -> CustodyPoolIdentityV1 {
        self.pool_identity
    }

    pub const fn custody_epoch_identity(&self) -> CustodyEpochIdentityV1 {
        self.custody_epoch_identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCustodyCommitteeV1 {
    pool_identity: CustodyPoolIdentityV1,
    authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
    committee: VerifiedCustodyEpochV1,
}

impl ValidatedCustodyCommitteeV1 {
    pub const fn pool_identity(&self) -> CustodyPoolIdentityV1 {
        self.pool_identity
    }

    pub const fn authorization_identity(&self) -> CustodyCommitteeAuthorizationIdentityV1 {
        self.authorization_identity
    }

    pub fn committee(&self) -> &VerifiedCustodyEpochV1 {
        &self.committee
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CustodyPoolError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    CustodyEpoch(#[from] CustodyEpochError),
    #[error("custody pool mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("unexpected custody pool issuer: {0}")]
    UnexpectedIssuer(&'static str),
    #[error("custody pool issuer signature is invalid")]
    InvalidPoolSignature,
    #[error("custody committee authorization signature is invalid")]
    InvalidCommitteeAuthorizationSignature,
    #[error("unknown committee node")]
    UnknownNode,
    #[error("custody pool member is not yet valid")]
    NotYetValid,
    #[error("custody pool member is expired")]
    Expired,
    #[error("custody pool member is revoked")]
    Revoked,
    #[error("custody pool policy violation: {0}")]
    PolicyViolation(&'static str),
}

pub fn validate_custody_epoch_against_pool_at(
    expected_policy_authority: CustodyEpochIssuerKeyV1,
    expected_authorization_identity: CustodyCommitteeAuthorizationIdentityV1,
    signed_pool: &SignedCustodyPoolV1,
    signed_epoch: &SignedCustodyEpochV1,
    signed_committee_authorization: &SignedCustodyCommitteeAuthorizationV1,
    now_unix_seconds: u64,
) -> Result<ValidatedCustodyCommitteeV1, CustodyPoolError> {
    let verified_pool = signed_pool.verify(expected_policy_authority)?;
    let verified_epoch = signed_epoch.verify()?;
    if verified_epoch.issuer() != expected_policy_authority {
        return Err(CustodyPoolError::UnexpectedIssuer("custody_epoch_issuer"));
    }
    let verified_authorization = signed_committee_authorization.verify(
        expected_policy_authority,
        expected_authorization_identity,
        verified_pool.pool_identity(),
        verified_epoch.epoch_identity(),
    )?;
    if verified_epoch.threshold().required() != 2 || verified_epoch.threshold().total() != 3 {
        return Err(CustodyPoolError::PolicyViolation("committee_threshold"));
    }

    let mut operators = HashSet::with_capacity(usize::from(verified_epoch.threshold().total()));
    let mut domains = HashSet::with_capacity(usize::from(verified_epoch.threshold().total()));
    for node in verified_epoch.nodes() {
        let member = verified_pool
            .member(node.node_public_key())
            .ok_or(CustodyPoolError::UnknownNode)?;
        if node.custody_public_key() != member.custody_public_key() {
            return Err(CustodyPoolError::BindingMismatch("node_custody_public_key"));
        }
        if verified_epoch.approved_suites() != member.approved_suites() {
            return Err(CustodyPoolError::BindingMismatch("approved_suites"));
        }
        validate_pool_member_active_at(member, now_unix_seconds)?;
        if !operators.insert(member.operator_id()) {
            return Err(CustodyPoolError::PolicyViolation("duplicate_operator"));
        }
        if !domains.insert(member.failure_domain_id()) {
            return Err(CustodyPoolError::PolicyViolation(
                "duplicate_failure_domain",
            ));
        }
    }

    Ok(ValidatedCustodyCommitteeV1 {
        pool_identity: verified_pool.pool_identity(),
        authorization_identity: verified_authorization.authorization_identity(),
        committee: verified_epoch,
    })
}

fn validate_pool_member_active_at(
    member: &CustodyPoolMemberV1,
    now_unix_seconds: u64,
) -> Result<(), CustodyPoolError> {
    if now_unix_seconds < member.valid_from() {
        return Err(CustodyPoolError::NotYetValid);
    }
    if now_unix_seconds >= member.valid_until() {
        return Err(CustodyPoolError::Expired);
    }
    if member.state() == CustodyPoolMemberStateV1::Revoked {
        return Err(CustodyPoolError::Revoked);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};

    use super::*;
    use crate::test_support::{
        digest, node_custody_public_key as shared_node_custody_public_key,
        node_public_key as shared_node_public_key, NOW,
    };
    use crate::{
        CustodyNodeIdentityV1, ShareCoordinateV1, ThresholdV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    };

    fn node_signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        shared_node_public_key(seed)
    }

    fn node_custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        shared_node_custody_public_key(seed)
    }

    fn operator_id(seed: u8) -> CustodyPoolOperatorIdV1 {
        CustodyPoolOperatorIdV1::new([seed; 32])
    }

    fn failure_domain_id(seed: u8) -> CustodyPoolFailureDomainIdV1 {
        CustodyPoolFailureDomainIdV1::new([seed; 32])
    }

    fn suites() -> CustodyApprovedSuitesV1 {
        CustodyApprovedSuitesV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        )
        .unwrap()
    }

    fn member(
        seed: u8,
        operator_seed: u8,
        failure_domain_seed: u8,
        active_window: (u64, u64),
        state: CustodyPoolMemberStateV1,
    ) -> CustodyPoolMemberV1 {
        member_with(
            seed,
            seed,
            operator_seed,
            failure_domain_seed,
            suites(),
            active_window,
            state,
        )
    }

    fn member_with(
        node_seed: u8,
        custody_seed: u8,
        operator_seed: u8,
        failure_domain_seed: u8,
        approved_suites: CustodyApprovedSuitesV1,
        active_window: (u64, u64),
        state: CustodyPoolMemberStateV1,
    ) -> CustodyPoolMemberV1 {
        CustodyPoolMemberV1::new(
            node_public_key(node_seed),
            node_custody_public_key(custody_seed),
            operator_id(operator_seed),
            failure_domain_id(failure_domain_seed),
            approved_suites,
            active_window,
            state,
        )
        .unwrap()
    }

    fn signed_pool_with_issuer(
        issuer_seed: u8,
        members: Vec<CustodyPoolMemberV1>,
    ) -> SignedCustodyPoolV1 {
        let issuer = SigningKey::from_bytes(&[issuer_seed; 32]);
        let statement = CustodyPoolStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer.verifying_key().to_bytes()).unwrap(),
            members,
        )
        .unwrap();
        SignedCustodyPoolV1::new(
            statement.clone(),
            issuer
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn signed_epoch_for_members(
        issuer_seed: u8,
        threshold: crate::ThresholdV1,
        members: &[(u8, u8)],
    ) -> SignedCustodyEpochV1 {
        let issuer = SigningKey::from_bytes(&[issuer_seed; 32]);
        let nodes = members
            .iter()
            .enumerate()
            .map(|(index, (node_seed, custody_seed))| {
                CustodyNodeIdentityV1::new(
                    node_public_key(*node_seed),
                    node_custody_public_key(*custody_seed),
                    ShareCoordinateV1::new(u8::try_from(index + 1).unwrap()).unwrap(),
                )
                .unwrap()
            })
            .collect();
        let statement = crate::CustodyEpochStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer.verifying_key().to_bytes()).unwrap(),
            suites(),
            threshold,
            nodes,
        )
        .unwrap();
        SignedCustodyEpochV1::new(
            statement.clone(),
            issuer
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn signed_committee_authorization_with_issuer(
        issuer_seed: u8,
        pool_identity: CustodyPoolIdentityV1,
        epoch_identity: CustodyEpochIdentityV1,
    ) -> SignedCustodyCommitteeAuthorizationV1 {
        let issuer = SigningKey::from_bytes(&[issuer_seed; 32]);
        let statement = CustodyCommitteeAuthorizationStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer.verifying_key().to_bytes()).unwrap(),
            pool_identity,
            epoch_identity,
        )
        .unwrap();
        SignedCustodyCommitteeAuthorizationV1::new(
            statement.clone(),
            issuer
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn committee_authorization_identity(
        authorization: &SignedCustodyCommitteeAuthorizationV1,
    ) -> CustodyCommitteeAuthorizationIdentityV1 {
        authorization.authorization_identity().unwrap()
    }

    #[test]
    fn custody_pool_validates_bound_two_of_three_committee() {
        let expected_issuer =
            CustodyEpochIssuerKeyV1::new(node_signing_key(0x71).verifying_key().to_bytes())
                .unwrap();
        let pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let epoch = signed_epoch_for_members(
            0x71,
            crate::ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );
        let authorization = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );

        let validated = validate_custody_epoch_against_pool_at(
            expected_issuer,
            committee_authorization_identity(&authorization),
            &pool,
            &epoch,
            &authorization,
            NOW,
        )
        .unwrap();

        assert_eq!(validated.pool_identity(), pool.pool_identity().unwrap());
        assert_eq!(
            validated.authorization_identity(),
            authorization.authorization_identity().unwrap()
        );
        assert_eq!(
            validated.committee().epoch_identity(),
            epoch.epoch_identity().unwrap()
        );
    }

    #[test]
    fn custody_pool_rejects_later_pool_and_fresh_authorization_substitution() {
        let expected_issuer =
            CustodyEpochIssuerKeyV1::new(node_signing_key(0x71).verifying_key().to_bytes())
                .unwrap();
        let original_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let later_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    9,
                    0xa9,
                    0xb9,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let epoch = signed_epoch_for_members(
            0x71,
            crate::ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );
        let original_authorization = signed_committee_authorization_with_issuer(
            0x71,
            original_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        let original_authorization_identity =
            committee_authorization_identity(&original_authorization);

        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                original_authorization_identity,
                &later_pool,
                &epoch,
                &original_authorization,
                NOW,
            ),
            Err(CustodyPoolError::BindingMismatch("custody_pool_identity"))
        );

        let later_authorization = signed_committee_authorization_with_issuer(
            0x71,
            later_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                original_authorization_identity,
                &later_pool,
                &epoch,
                &later_authorization,
                NOW,
            ),
            Err(CustodyPoolError::BindingMismatch(
                "custody_committee_authorization_identity"
            ))
        );
    }

    #[test]
    fn custody_pool_rejects_committee_node_absent_from_exact_pool() {
        let now_unix_seconds = 2_000_000_000;
        let expected_issuer =
            CustodyEpochIssuerKeyV1::new(node_signing_key(0x71).verifying_key().to_bytes())
                .unwrap();
        let epoch = signed_epoch_for_members(
            0x71,
            ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );

        let missing_member_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let missing_member_authorization = signed_committee_authorization_with_issuer(
            0x71,
            missing_member_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&missing_member_authorization),
                &missing_member_pool,
                &epoch,
                &missing_member_authorization,
                now_unix_seconds,
            ),
            Err(CustodyPoolError::UnknownNode)
        );
    }

    #[test]
    fn custody_pool_rejects_inactive_revoked_members_and_duplicate_domains() {
        let now_unix_seconds = 2_000_000_000;
        let expected_issuer =
            CustodyEpochIssuerKeyV1::new(node_signing_key(0x71).verifying_key().to_bytes())
                .unwrap();
        let epoch = signed_epoch_for_members(
            0x71,
            ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );

        let not_yet_valid_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (now_unix_seconds + 1, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let not_yet_valid_authorization = signed_committee_authorization_with_issuer(
            0x71,
            not_yet_valid_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&not_yet_valid_authorization),
                &not_yet_valid_pool,
                &epoch,
                &not_yet_valid_authorization,
                now_unix_seconds,
            ),
            Err(CustodyPoolError::NotYetValid)
        );

        let expired_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (now_unix_seconds - 10, now_unix_seconds),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let expired_authorization = signed_committee_authorization_with_issuer(
            0x71,
            expired_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&expired_authorization),
                &expired_pool,
                &epoch,
                &expired_authorization,
                now_unix_seconds,
            ),
            Err(CustodyPoolError::Expired)
        );

        let revoked_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Revoked,
                ),
            ],
        );
        let revoked_authorization = signed_committee_authorization_with_issuer(
            0x71,
            revoked_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&revoked_authorization),
                &revoked_pool,
                &epoch,
                &revoked_authorization,
                now_unix_seconds,
            ),
            Err(CustodyPoolError::Revoked)
        );

        let duplicate_operator_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa1,
                    0xb2,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let duplicate_operator_authorization = signed_committee_authorization_with_issuer(
            0x71,
            duplicate_operator_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&duplicate_operator_authorization),
                &duplicate_operator_pool,
                &epoch,
                &duplicate_operator_authorization,
                now_unix_seconds,
            ),
            Err(CustodyPoolError::PolicyViolation("duplicate_operator"))
        );

        let duplicate_domain_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb1,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (now_unix_seconds - 10, now_unix_seconds + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let duplicate_domain_authorization = signed_committee_authorization_with_issuer(
            0x71,
            duplicate_domain_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&duplicate_domain_authorization),
                &duplicate_domain_pool,
                &epoch,
                &duplicate_domain_authorization,
                now_unix_seconds,
            ),
            Err(CustodyPoolError::PolicyViolation(
                "duplicate_failure_domain"
            ))
        );
    }

    #[test]
    fn custody_pool_rejects_binding_threshold_and_issuer_mismatches() {
        let expected_issuer =
            CustodyEpochIssuerKeyV1::new(node_signing_key(0x71).verifying_key().to_bytes())
                .unwrap();
        let pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let epoch = signed_epoch_for_members(
            0x71,
            crate::ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );
        let authorization = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );

        let wrong_custody_key_pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member_with(
                    2,
                    9,
                    0xa2,
                    0xb2,
                    suites(),
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let custody_key_authorization = signed_committee_authorization_with_issuer(
            0x71,
            wrong_custody_key_pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&custody_key_authorization),
                &wrong_custody_key_pool,
                &epoch,
                &custody_key_authorization,
                NOW,
            ),
            Err(CustodyPoolError::BindingMismatch("node_custody_public_key"))
        );

        let mut wrong_suites_pool_bytes = pool.canonical_bytes().unwrap();
        let suite = CUSTODY_X_WING_AES256GCM_SUITE_ID_V1.as_bytes();
        let suite_index = wrong_suites_pool_bytes
            .windows(suite.len())
            .position(|window| window == suite)
            .unwrap();
        wrong_suites_pool_bytes[suite_index + suite.len() - 1] = b'2';
        assert_eq!(
            SignedCustodyPoolV1::from_canonical_bytes(&wrong_suites_pool_bytes),
            Err(ContractError::InvalidField("recipient_encryption_suite_id"))
        );

        let mut wrong_epoch_suites_bytes = epoch.canonical_bytes().unwrap();
        let suite_index = wrong_epoch_suites_bytes
            .windows(suite.len())
            .position(|window| window == suite)
            .unwrap();
        wrong_epoch_suites_bytes[suite_index + suite.len() - 1] = b'2';
        assert_eq!(
            SignedCustodyEpochV1::from_canonical_bytes(&wrong_epoch_suites_bytes),
            Err(ContractError::InvalidField("recipient_encryption_suite_id"))
        );

        let wrong_threshold_epoch = signed_epoch_for_members(
            0x71,
            crate::ThresholdV1::new(3, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );
        let wrong_threshold_authorization = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            wrong_threshold_epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&wrong_threshold_authorization),
                &pool,
                &wrong_threshold_epoch,
                &wrong_threshold_authorization,
                NOW,
            ),
            Err(CustodyPoolError::PolicyViolation("committee_threshold"))
        );

        let wrong_pool_authorization = signed_committee_authorization_with_issuer(
            0x71,
            CustodyPoolIdentityV1::new(digest(0xee), 123).unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&wrong_pool_authorization),
                &pool,
                &epoch,
                &wrong_pool_authorization,
                NOW,
            ),
            Err(CustodyPoolError::BindingMismatch("custody_pool_identity"))
        );

        let wrong_epoch_authorization = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            CustodyEpochIdentityV1::new(digest(0xef), 456).unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&wrong_epoch_authorization),
                &pool,
                &epoch,
                &wrong_epoch_authorization,
                NOW,
            ),
            Err(CustodyPoolError::BindingMismatch("custody_epoch_identity"))
        );

        let mut bad_signature = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        )
        .canonical_bytes()
        .unwrap();
        *bad_signature.last_mut().unwrap() ^= 1;
        let bad_signature =
            SignedCustodyCommitteeAuthorizationV1::from_canonical_bytes(&bad_signature).unwrap();
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&bad_signature),
                &pool,
                &epoch,
                &bad_signature,
                NOW,
            ),
            Err(CustodyPoolError::InvalidCommitteeAuthorizationSignature)
        );

        let mut bad_pool_signature = pool.canonical_bytes().unwrap();
        *bad_pool_signature.last_mut().unwrap() ^= 1;
        let bad_pool_signature =
            SignedCustodyPoolV1::from_canonical_bytes(&bad_pool_signature).unwrap();
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&authorization),
                &bad_pool_signature,
                &epoch,
                &authorization,
                NOW,
            ),
            Err(CustodyPoolError::InvalidPoolSignature)
        );

        let wrong_expected_issuer =
            CustodyEpochIssuerKeyV1::new(node_signing_key(0x88).verifying_key().to_bytes())
                .unwrap();
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                wrong_expected_issuer,
                committee_authorization_identity(&authorization),
                &pool,
                &epoch,
                &authorization,
                NOW,
            ),
            Err(CustodyPoolError::UnexpectedIssuer("custody_pool_issuer"))
        );

        let alternate_epoch = signed_epoch_for_members(
            0x88,
            crate::ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );
        let epoch_issuer_mismatch_authorization = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            alternate_epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&epoch_issuer_mismatch_authorization),
                &pool,
                &alternate_epoch,
                &epoch_issuer_mismatch_authorization,
                NOW,
            ),
            Err(CustodyPoolError::UnexpectedIssuer("custody_epoch_issuer"))
        );

        let authorization_issuer_mismatch = signed_committee_authorization_with_issuer(
            0x88,
            pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );
        assert_eq!(
            validate_custody_epoch_against_pool_at(
                expected_issuer,
                committee_authorization_identity(&authorization_issuer_mismatch),
                &pool,
                &epoch,
                &authorization_issuer_mismatch,
                NOW,
            ),
            Err(CustodyPoolError::UnexpectedIssuer(
                "custody_committee_authorization_issuer"
            ))
        );
    }

    #[test]
    fn custody_pool_public_types_expose_only_policy_and_authority_fields() {
        let pool = signed_pool_with_issuer(
            0x71,
            vec![
                member(
                    1,
                    0xa1,
                    0xb1,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    2,
                    0xa2,
                    0xb2,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
                member(
                    3,
                    0xa3,
                    0xb3,
                    (NOW - 10, NOW + 10),
                    CustodyPoolMemberStateV1::Active,
                ),
            ],
        );
        let epoch = signed_epoch_for_members(
            0x71,
            ThresholdV1::new(2, 3).unwrap(),
            &[(1, 1), (2, 2), (3, 3)],
        );
        let authorization = signed_committee_authorization_with_issuer(
            0x71,
            pool.pool_identity().unwrap(),
            epoch.epoch_identity().unwrap(),
        );

        assert_eq!(
            authorization.statement().pool_identity(),
            pool.pool_identity().unwrap()
        );
        assert_eq!(
            authorization.statement().custody_epoch_identity(),
            epoch.epoch_identity().unwrap()
        );

        let debug = format!("{pool:?}\n{authorization:?}").to_ascii_lowercase();
        for forbidden in [
            "route:",
            "address:",
            "ip:",
            "hostname:",
            "url:",
            "socket:",
            "transport:",
            "endpoint:",
            "port:",
            "wireguard",
            "alpn",
        ] {
            assert!(!debug.contains(forbidden));
        }

        let operator = operator_id(0xa1);
        let domain = failure_domain_id(0xb1);
        assert_eq!(operator.as_bytes(), &[0xa1; 32]);
        assert_eq!(domain.as_bytes(), &[0xb1; 32]);
    }
}
