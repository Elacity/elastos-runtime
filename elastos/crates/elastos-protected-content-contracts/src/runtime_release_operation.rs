use ed25519_dalek::{Signature, Verifier as _};
use serde::Serialize;
use thiserror::Error;

use crate::canonical::{CanonicalBody, ContractError, Decoder, Encoder};
use crate::rights::{validate_active, validate_time_window, RightsError};
use crate::{
    CanonicalContract, CustodyEnvelopeV1, CustodyEpochError, CustodyEpochIdentityV1, Digest32,
    KeyReleaseError, KeyReleaseRequestV1, NodePublicKey, NodeSetV1, RecipientAuthorizationError,
    RecipientKeyAuthorizationContextV1, RecipientPublicKeyBytesV1, ReplayClaimKeyV1,
    RightsEvaluationEvidenceRequestV1, RightsPolicyBodyV1, RuntimeOperationIssuerKeyV1,
    SignedCustodyEpochV1, SignedNodeContributionV1, SignedNodeRightsDecisionV1,
    SignedRecipientKeyAuthorizationV1, SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
    VerifiedKeyReleaseRequestV1, VerifiedNodeContributionV1, VerifiedNodeRightsDecisionV1,
    WalletSignedRightsRequestV1,
};

pub const MAX_RUNTIME_RELEASE_OPERATION_LIFETIME_SECS: u64 = 60;
const ED25519_SIGNATURE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RuntimeReleaseAuditIdV1(Digest32);

impl RuntimeReleaseAuditIdV1 {
    pub fn new(value: Digest32) -> Result<Self, ContractError> {
        if value == Digest32::new([0; 32]) {
            return Err(ContractError::InvalidField("runtime_release_audit_id"));
        }
        Ok(Self(value))
    }

    pub const fn digest(&self) -> Digest32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeReleaseOperationError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error(transparent)]
    Rights(#[from] RightsError),
    #[error(transparent)]
    KeyRelease(#[from] KeyReleaseError),
    #[error(transparent)]
    RecipientAuthorization(#[from] RecipientAuthorizationError),
    #[error(transparent)]
    CustodyEpoch(#[from] CustodyEpochError),
    #[error("runtime release operation mismatch: {0}")]
    BindingMismatch(&'static str),
    #[error("runtime release operation signature is invalid")]
    InvalidRuntimeSignature,
    #[error("runtime release operation is not yet valid")]
    NotYetValid,
    #[error("runtime release operation expired")]
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReleaseOperationStatementV1 {
    runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
    rights_request: WalletSignedRightsRequestV1,
    release_request: KeyReleaseRequestV1,
    recipient_public_key: RecipientPublicKeyBytesV1,
    recipient_authorization: SignedRecipientKeyAuthorizationV1,
    policy_body: RightsPolicyBodyV1,
    evidence_request: RightsEvaluationEvidenceRequestV1,
    custody_epoch: SignedCustodyEpochV1,
    audit_request_id: RuntimeReleaseAuditIdV1,
    issued_at: u64,
    expires_at: u64,
}

impl RuntimeReleaseOperationStatementV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
        rights_request: WalletSignedRightsRequestV1,
        release_request: KeyReleaseRequestV1,
        recipient_public_key: RecipientPublicKeyBytesV1,
        recipient_authorization: SignedRecipientKeyAuthorizationV1,
        policy_body: RightsPolicyBodyV1,
        evidence_request: RightsEvaluationEvidenceRequestV1,
        custody_epoch: SignedCustodyEpochV1,
        audit_request_id: RuntimeReleaseAuditIdV1,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            runtime_operation_issuer,
            rights_request,
            release_request,
            recipient_public_key,
            recipient_authorization,
            policy_body,
            evidence_request,
            custody_epoch,
            audit_request_id,
            issued_at,
            expires_at,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn runtime_operation_issuer(&self) -> RuntimeOperationIssuerKeyV1 {
        self.runtime_operation_issuer
    }

    pub fn rights_request(&self) -> &WalletSignedRightsRequestV1 {
        &self.rights_request
    }

    pub fn release_request(&self) -> &KeyReleaseRequestV1 {
        &self.release_request
    }

    pub const fn recipient_public_key(&self) -> RecipientPublicKeyBytesV1 {
        self.recipient_public_key
    }

    pub fn recipient_authorization(&self) -> &SignedRecipientKeyAuthorizationV1 {
        &self.recipient_authorization
    }

    pub fn policy_body(&self) -> &RightsPolicyBodyV1 {
        &self.policy_body
    }

    pub fn evidence_request(&self) -> &RightsEvaluationEvidenceRequestV1 {
        &self.evidence_request
    }

    pub fn custody_epoch(&self) -> &SignedCustodyEpochV1 {
        &self.custody_epoch
    }

    pub const fn audit_request_id(&self) -> RuntimeReleaseAuditIdV1 {
        self.audit_request_id
    }

    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl CanonicalBody for RuntimeReleaseOperationStatementV1 {
    const DOMAIN: &'static str = "elastos.protected-content.runtime-release-operation-statement/v1";

    fn validate(&self) -> Result<(), ContractError> {
        RuntimeOperationIssuerKeyV1::new(*self.runtime_operation_issuer.as_bytes())?;
        self.rights_request.canonical_bytes()?;
        self.release_request.canonical_bytes()?;
        RecipientPublicKeyBytesV1::new(*self.recipient_public_key.as_bytes())?;
        self.recipient_authorization.canonical_bytes()?;
        self.policy_body.canonical_bytes()?;
        self.evidence_request.canonical_bytes()?;
        self.evidence_request
            .validate_against_policy(&self.policy_body)?;
        self.custody_epoch.canonical_bytes()?;
        RuntimeReleaseAuditIdV1::new(self.audit_request_id.digest())?;
        validate_time_window(
            self.issued_at,
            self.expires_at,
            MAX_RUNTIME_RELEASE_OPERATION_LIFETIME_SECS,
            "runtime_release_operation_lifetime",
        )?;

        if self.rights_request.request().binding() != self.release_request.binding() {
            return Err(ContractError::InvalidField("release_request.binding"));
        }
        if self.rights_request.request().request_hash()?
            != self.release_request.rights_request_hash()
        {
            return Err(ContractError::InvalidField(
                "release_request.rights_request_hash",
            ));
        }
        if self.rights_request.request().action() != self.release_request.action() {
            return Err(ContractError::InvalidField("release_request.action"));
        }
        if self.rights_request.request().recipient() != self.release_request.recipient() {
            return Err(ContractError::InvalidField("release_request.recipient"));
        }
        if self.rights_request.request().binding() != self.evidence_request.binding() {
            return Err(ContractError::InvalidField("evidence_request.binding"));
        }
        if self.rights_request.request().binding()
            != self.recipient_authorization.statement().binding()
        {
            return Err(ContractError::InvalidField(
                "recipient_authorization.binding",
            ));
        }
        if self.rights_request.request().action()
            != self.recipient_authorization.statement().action()
        {
            return Err(ContractError::InvalidField(
                "recipient_authorization.action",
            ));
        }
        if self.release_request.recipient()
            != self
                .recipient_authorization
                .statement()
                .recipient_identity()
        {
            return Err(ContractError::InvalidField(
                "recipient_authorization.recipient_key_identity",
            ));
        }
        if self.recipient_public_key
            != self
                .recipient_authorization
                .statement()
                .recipient_public_key()
        {
            return Err(ContractError::InvalidField(
                "recipient_authorization.recipient_public_key",
            ));
        }
        if self.runtime_operation_issuer
            != self
                .recipient_authorization
                .statement()
                .runtime_operation_issuer()
        {
            return Err(ContractError::InvalidField(
                "recipient_authorization.runtime_operation_issuer",
            ));
        }
        if !self
            .release_request
            .recipient()
            .matches_public_key(self.recipient_public_key.as_bytes())
        {
            return Err(ContractError::InvalidField("recipient_public_key"));
        }
        if self.policy_body.required_action() != self.release_request.action() {
            return Err(ContractError::InvalidField("policy_body.required_action"));
        }
        if self.policy_body.policy_identity()? != *self.release_request.binding().rights_policy() {
            return Err(ContractError::InvalidField("policy_body"));
        }
        if self.issued_at < self.release_request.issued_at()
            || self.expires_at > self.release_request.expires_at()
        {
            return Err(ContractError::InvalidField(
                "runtime_release_operation_lifetime",
            ));
        }
        if self.issued_at < self.recipient_authorization.statement().issued_at()
            || self.expires_at > self.recipient_authorization.statement().expires_at()
        {
            return Err(ContractError::InvalidField(
                "runtime_release_operation_lifetime",
            ));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.fixed(self.runtime_operation_issuer.as_bytes());
        encoder.nested(&self.rights_request)?;
        encoder.nested(&self.release_request)?;
        encoder.fixed(self.recipient_public_key.as_bytes());
        encoder.nested(&self.recipient_authorization)?;
        encoder.nested(&self.policy_body)?;
        encoder.nested(&self.evidence_request)?;
        encoder.nested(&self.custody_epoch)?;
        encoder.fixed(self.audit_request_id.digest().as_bytes());
        encoder.u64(self.issued_at);
        encoder.u64(self.expires_at);
        Ok(())
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            RuntimeOperationIssuerKeyV1::new(decoder.fixed()?)?,
            decoder.nested("rights_request")?,
            decoder.nested("release_request")?,
            RecipientPublicKeyBytesV1::new(decoder.fixed()?)?,
            decoder.nested("recipient_authorization")?,
            decoder.nested("policy_body")?,
            decoder.nested("evidence_request")?,
            decoder.nested("custody_epoch")?,
            RuntimeReleaseAuditIdV1::new(Digest32::new(decoder.fixed()?))?,
            decoder.u64()?,
            decoder.u64()?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRuntimeReleaseOperationV1 {
    statement: RuntimeReleaseOperationStatementV1,
    runtime_signature: Vec<u8>,
}

impl SignedRuntimeReleaseOperationV1 {
    pub fn new(
        statement: RuntimeReleaseOperationStatementV1,
        runtime_signature: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            statement,
            runtime_signature,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn statement(&self) -> &RuntimeReleaseOperationStatementV1 {
        &self.statement
    }

    pub fn verify(
        &self,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now: u64,
    ) -> Result<AuthenticatedRuntimeReleaseOperationV1, RuntimeReleaseOperationError> {
        self.canonical_bytes()?;
        map_active(self.statement.issued_at, self.statement.expires_at, now)?;
        if self.statement.runtime_operation_issuer != expected_runtime_issuer {
            return Err(RuntimeReleaseOperationError::BindingMismatch(
                "runtime_operation_issuer",
            ));
        }
        let signature = Signature::from_bytes(
            &self
                .runtime_signature
                .clone()
                .try_into()
                .map_err(|_| RuntimeReleaseOperationError::InvalidRuntimeSignature)?,
        );
        let runtime_key = crate::identity::validate_ed25519_public_key(
            *self.statement.runtime_operation_issuer.as_bytes(),
            "runtime_operation_issuer",
        )
        .map_err(|_| RuntimeReleaseOperationError::InvalidRuntimeSignature)?;
        runtime_key
            .verify(&self.statement.canonical_bytes()?, &signature)
            .map_err(|_| RuntimeReleaseOperationError::InvalidRuntimeSignature)?;
        let verified = verify_release_operation_statement(&self.statement, now)?;
        Ok(AuthenticatedRuntimeReleaseOperationV1 {
            statement: self.statement.clone(),
            operation_hash: self.statement.canonical_hash()?,
            rights_request_hash: self.statement.rights_request.request().request_hash()?,
            rights_request_replay_claim_key: self
                .statement
                .rights_request
                .request()
                .replay_claim_key()?,
            release_request_hash: verified.release.request_hash(),
            release_request_replay_claim_key: self.statement.release_request.replay_claim_key()?,
            recipient_authorization_hash: self
                .statement
                .recipient_authorization
                .statement()
                .canonical_hash()?,
            custody_epoch_identity: verified.epoch.epoch_identity(),
        })
    }
}

impl CanonicalBody for SignedRuntimeReleaseOperationV1 {
    const DOMAIN: &'static str = "elastos.protected-content.runtime-release-operation/v1";

    fn validate(&self) -> Result<(), ContractError> {
        self.statement.canonical_bytes()?;
        if self.runtime_signature.len() != ED25519_SIGNATURE_BYTES {
            return Err(ContractError::InvalidField("runtime_signature"));
        }
        Ok(())
    }

    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError> {
        encoder.nested(&self.statement)?;
        encoder.bytes(&self.runtime_signature)
    }

    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError> {
        Self::new(
            decoder.nested("statement")?,
            decoder.bytes("runtime_signature", ED25519_SIGNATURE_BYTES)?,
        )
    }
}

/// Signature-checked Runtime release operation with exact replay claim keys.
///
/// This type is deliberately replay-pending. It proves the Runtime signature,
/// nested request signatures, recipient authorization, and custody epoch
/// bindings, but it does not claim replay and it does not expose actionable
/// `VerifiedRightsRequestV1` or `VerifiedKeyReleaseRequestV1` values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedRuntimeReleaseOperationV1 {
    statement: RuntimeReleaseOperationStatementV1,
    operation_hash: Digest32,
    rights_request_hash: Digest32,
    rights_request_replay_claim_key: ReplayClaimKeyV1,
    release_request_hash: Digest32,
    release_request_replay_claim_key: ReplayClaimKeyV1,
    recipient_authorization_hash: Digest32,
    custody_epoch_identity: CustodyEpochIdentityV1,
}

impl AuthenticatedRuntimeReleaseOperationV1 {
    pub fn statement(&self) -> &RuntimeReleaseOperationStatementV1 {
        &self.statement
    }

    pub const fn operation_hash(&self) -> Digest32 {
        self.operation_hash
    }

    pub const fn rights_request_hash(&self) -> Digest32 {
        self.rights_request_hash
    }

    pub const fn rights_request_replay_claim_key(&self) -> ReplayClaimKeyV1 {
        self.rights_request_replay_claim_key
    }

    pub const fn release_request_hash(&self) -> Digest32 {
        self.release_request_hash
    }

    pub const fn release_request_replay_claim_key(&self) -> ReplayClaimKeyV1 {
        self.release_request_replay_claim_key
    }

    pub const fn recipient_authorization_hash(&self) -> Digest32 {
        self.recipient_authorization_hash
    }

    pub const fn custody_epoch_identity(&self) -> CustodyEpochIdentityV1 {
        self.custody_epoch_identity
    }

    pub fn binding(&self) -> &crate::ProtectedContentBindingV1 {
        self.statement.release_request().binding()
    }

    pub fn action(&self) -> crate::RightsActionV1 {
        self.statement.release_request().action()
    }

    pub fn recipient(&self) -> &crate::RecipientKeyIdentityV1 {
        self.statement.release_request().recipient()
    }

    pub fn validate_node_release_claim_context(
        &self,
        envelope: &CustodyEnvelopeV1,
        node_public_key: NodePublicKey,
        now: u64,
    ) -> Result<(), RuntimeReleaseOperationError> {
        map_active(self.statement.issued_at, self.statement.expires_at, now)?;
        let verified = verify_release_operation_statement(&self.statement, now)?;
        if !envelope.matches_key_envelope_identity(verified.release.binding().key_envelope())? {
            return Err(RuntimeReleaseOperationError::BindingMismatch(
                "key_envelope",
            ));
        }
        if envelope.manifest().custody_epoch() != verified.epoch.epoch_identity() {
            return Err(RuntimeReleaseOperationError::BindingMismatch(
                "custody_epoch",
            ));
        }
        if envelope.manifest().node(node_public_key).is_none() {
            return Err(RuntimeReleaseOperationError::BindingMismatch(
                "custody_node",
            ));
        }
        Ok(())
    }

    pub fn verify_node_rights_decision(
        &self,
        decision: &SignedNodeRightsDecisionV1,
        node_set: &NodeSetV1,
        now: u64,
    ) -> Result<VerifiedNodeRightsDecisionV1, KeyReleaseError> {
        let verified = verify_release_operation_statement(&self.statement, now)
            .map_err(map_runtime_release_key_release_error)?;
        decision.verify(&verified.release, node_set, now)
    }

    pub fn validate_node_contribution_active_window(
        &self,
        issued_at: u64,
        expires_at: u64,
        decision: &VerifiedNodeRightsDecisionV1,
        now: u64,
    ) -> Result<(), KeyReleaseError> {
        let verified = verify_release_operation_statement(&self.statement, now)
            .map_err(map_runtime_release_key_release_error)?;
        crate::validate_node_contribution_active_window(
            issued_at,
            expires_at,
            &verified.release,
            decision,
            now,
        )
    }

    pub fn verify_node_contribution(
        &self,
        contribution: &SignedNodeContributionV1,
        node_set: &NodeSetV1,
        now: u64,
    ) -> Result<VerifiedNodeContributionV1, KeyReleaseError> {
        let verified = verify_release_operation_statement(&self.statement, now)
            .map_err(map_runtime_release_key_release_error)?;
        contribution.verify(&verified.release, node_set, now)
    }

    pub fn verify_terminal_receipt(
        &self,
        terminal_receipt: &SignedTerminalReceiptV1,
        contributions: &[VerifiedNodeContributionV1],
        expected_issuer: TerminalReceiptIssuerKey,
        now: u64,
    ) -> Result<(), KeyReleaseError> {
        let verified = verify_release_operation_statement(&self.statement, now)
            .map_err(map_runtime_release_key_release_error)?;
        terminal_receipt.verify(&verified.release, contributions, expected_issuer, now)
    }
}

fn map_active(
    issued_at: u64,
    expires_at: u64,
    now: u64,
) -> Result<(), RuntimeReleaseOperationError> {
    match validate_active(issued_at, expires_at, now) {
        Ok(()) => Ok(()),
        Err(RightsError::NotYetValid) => Err(RuntimeReleaseOperationError::NotYetValid),
        Err(RightsError::Expired) => Err(RuntimeReleaseOperationError::Expired),
        Err(RightsError::Contract(error)) => Err(RuntimeReleaseOperationError::Contract(error)),
        Err(other) => Err(RuntimeReleaseOperationError::Rights(other)),
    }
}

struct VerifiedReleaseOperationStatementV1 {
    release: VerifiedKeyReleaseRequestV1,
    epoch: crate::VerifiedCustodyEpochV1,
}

fn verify_release_operation_statement(
    statement: &RuntimeReleaseOperationStatementV1,
    now: u64,
) -> Result<VerifiedReleaseOperationStatementV1, RuntimeReleaseOperationError> {
    let rights_context = crate::RightsVerificationContextV1::new(
        statement.rights_request.request().binding().clone(),
        statement.rights_request.request().action(),
        statement.rights_request.request().recipient().clone(),
        now,
    );
    let verified_rights = statement.rights_request.verify_unclaimed(&rights_context)?;
    let verified_release = statement
        .release_request
        .verify_unclaimed(&verified_rights, now)?;
    let recipient_context = RecipientKeyAuthorizationContextV1::new(
        verified_release.binding().clone(),
        verified_release.action(),
        statement.recipient_public_key,
        statement.runtime_operation_issuer,
        now,
    );
    let verified_authorization = statement
        .recipient_authorization
        .verify(&recipient_context)?;
    let verified_epoch = statement
        .custody_epoch
        .verify_against_key_envelope(verified_release.binding().key_envelope())?;
    if verified_epoch
        .approved_suites()
        .recipient_encryption_suite_id()
        != verified_authorization
            .recipient_identity()
            .encryption_suite_id()
    {
        return Err(RuntimeReleaseOperationError::BindingMismatch(
            "recipient_encryption_suite_id",
        ));
    }
    if statement.recipient_public_key.key_identity(
        verified_epoch
            .approved_suites()
            .recipient_encryption_suite_id(),
    )? != *verified_authorization.recipient_identity()
    {
        return Err(RuntimeReleaseOperationError::BindingMismatch(
            "recipient_public_key",
        ));
    }
    Ok(VerifiedReleaseOperationStatementV1 {
        release: verified_release,
        epoch: verified_epoch,
    })
}

fn map_runtime_release_key_release_error(error: RuntimeReleaseOperationError) -> KeyReleaseError {
    match error {
        RuntimeReleaseOperationError::Contract(error) => KeyReleaseError::Contract(error),
        RuntimeReleaseOperationError::Rights(error) => KeyReleaseError::Rights(error),
        RuntimeReleaseOperationError::KeyRelease(error) => error,
        RuntimeReleaseOperationError::RecipientAuthorization(_) => {
            KeyReleaseError::BindingMismatch("recipient_authorization")
        }
        RuntimeReleaseOperationError::CustodyEpoch(_) => {
            KeyReleaseError::BindingMismatch("custody_epoch")
        }
        RuntimeReleaseOperationError::BindingMismatch(field) => {
            KeyReleaseError::BindingMismatch(field)
        }
        RuntimeReleaseOperationError::InvalidRuntimeSignature => {
            KeyReleaseError::BindingMismatch("runtime_signature")
        }
        RuntimeReleaseOperationError::NotYetValid => {
            KeyReleaseError::Rights(RightsError::NotYetValid)
        }
        RuntimeReleaseOperationError::Expired => KeyReleaseError::Rights(RightsError::Expired),
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};
    use hex::encode;
    use k256::ecdsa::SigningKey as WalletSigningKey;
    use sha3::Digest as _;

    use elastos_auth::ethereum_signed_message_hash;

    use super::*;
    use crate::test_support::{digest, NOW};
    use crate::{
        CustodyApprovedSuitesV1, CustodyEpochIssuerKeyV1, CustodyEpochStatementV1,
        CustodyNodeIdentityV1, EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1,
        NodeCustodyPublicKeyV1, NodePublicKey, ProfileIdentityV1, ProtectedContentBindingV1,
        ReplayNonce16, RightsActionV1, RightsObservationFinalityV1, RightsSubjectSourceV1,
        RuntimeSessionBindingV1, ShareCoordinateV1, ThresholdV1, WalletAddress,
    };

    fn wallet(seed: u8) -> WalletAddress {
        let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
        let encoded = key.verifying_key().to_encoded_point(false);
        let digest = sha3::Keccak256::digest(&encoded.as_bytes()[1..]);
        WalletAddress::new(digest[12..].try_into().unwrap())
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        let key = SigningKey::from_bytes(&[seed; 32]);
        NodePublicKey::new(key.verifying_key().to_bytes()).unwrap()
    }

    fn custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        crate::test_support::node_custody_public_key(seed)
    }

    fn signed_epoch() -> SignedCustodyEpochV1 {
        let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
        let statement = CustodyEpochStatementV1::new(
            CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
            CustodyApprovedSuitesV1::new(
                crate::CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                crate::CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
                crate::CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            )
            .unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            vec![
                CustodyNodeIdentityV1::new(
                    node_public_key(1),
                    custody_public_key(1),
                    ShareCoordinateV1::new(1).unwrap(),
                )
                .unwrap(),
                CustodyNodeIdentityV1::new(
                    node_public_key(2),
                    custody_public_key(2),
                    ShareCoordinateV1::new(2).unwrap(),
                )
                .unwrap(),
                CustodyNodeIdentityV1::new(
                    node_public_key(3),
                    custody_public_key(3),
                    ShareCoordinateV1::new(3).unwrap(),
                )
                .unwrap(),
            ],
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

    fn envelope_for_epoch(custody_epoch: CustodyEpochIdentityV1) -> CustodyEnvelopeV1 {
        let manifest = crate::CustodyEnvelopeManifestV1::new(
            crate::EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
            crate::CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap(),
            custody_epoch,
            crate::CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap(),
            ThresholdV1::new(2, 3).unwrap(),
            digest(0x19),
            vec![
                CustodyNodeIdentityV1::new(
                    node_public_key(1),
                    custody_public_key(1),
                    ShareCoordinateV1::new(1).unwrap(),
                )
                .unwrap(),
                CustodyNodeIdentityV1::new(
                    node_public_key(2),
                    custody_public_key(2),
                    ShareCoordinateV1::new(2).unwrap(),
                )
                .unwrap(),
                CustodyNodeIdentityV1::new(
                    node_public_key(3),
                    custody_public_key(3),
                    ShareCoordinateV1::new(3).unwrap(),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let stored_share = crate::test_support::sealed_share(0x31);
        CustodyEnvelopeV1::new(
            manifest,
            vec![
                stored_share.clone(),
                stored_share.clone(),
                stored_share.clone(),
            ],
        )
        .unwrap()
    }

    fn policy_body() -> RightsPolicyBodyV1 {
        RightsPolicyBodyV1::new(
            crate::EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
            crate::ContentAccessIdV1::new([0x41; 16]).unwrap(),
            RightsActionV1::View,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap()
    }

    fn binding() -> ProtectedContentBindingV1 {
        let encrypted_content = crate::EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap();
        let envelope = envelope_for_epoch(signed_epoch().epoch_identity().unwrap())
            .key_envelope_identity()
            .unwrap();
        let profile = SigningKey::from_bytes(&[0x26; 32]);
        let policy_body = policy_body();
        ProtectedContentBindingV1::new(
            encrypted_content,
            envelope,
            policy_body.policy_identity().unwrap(),
            ProfileIdentityV1::from_public_key_bytes(profile.verifying_key().to_bytes()).unwrap(),
            wallet(7),
            RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
        )
        .unwrap()
    }

    fn signed_operation() -> SignedRuntimeReleaseOperationV1 {
        let runtime_key = SigningKey::from_bytes(&[0x42; 32]);
        let recipient_public_key = crate::test_support::recipient_public_key_bytes(9);
        let binding = binding();
        let policy_body = policy_body();
        let rights_request = crate::RightsRequestV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_public_key
                .key_identity(crate::CUSTODY_X_WING_AES256GCM_SUITE_ID_V1)
                .unwrap(),
            NOW,
            NOW + 180,
            ReplayNonce16::new([0x55; 16]),
        )
        .unwrap();
        let wallet_key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
        let (wallet_signature, recovery_id) = wallet_key
            .sign_prehash_recoverable(&ethereum_signed_message_hash(
                &rights_request.canonical_bytes().unwrap(),
            ))
            .unwrap();
        let mut wallet_signature_bytes = wallet_signature.to_bytes().to_vec();
        wallet_signature_bytes.push(recovery_id.to_byte());
        let rights_request =
            WalletSignedRightsRequestV1::new(rights_request, wallet_signature_bytes).unwrap();
        let release_request = KeyReleaseRequestV1::new(
            binding.clone(),
            rights_request.request().request_hash().unwrap(),
            RightsActionV1::View,
            rights_request.request().recipient().clone(),
            NOW + 1,
            NOW + 50,
            ReplayNonce16::new([0x66; 16]),
        )
        .unwrap();
        let profile = SigningKey::from_bytes(&[0x26; 32]);
        let authorization_statement = crate::RecipientKeyAuthorizationStatementV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_public_key,
            rights_request.request().recipient().clone(),
            RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
            NOW,
            NOW + 90,
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
        let statement = RuntimeReleaseOperationStatementV1::new(
            RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
            rights_request,
            release_request,
            recipient_public_key,
            authorization,
            policy_body.clone(),
            RightsEvaluationEvidenceRequestV1::new(binding, policy_body.policy_identity().unwrap())
                .unwrap(),
            signed_epoch(),
            RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
            NOW + 2,
            NOW + 40,
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

    #[test]
    fn runtime_release_operation_binds_exact_authority_hashes_and_replay_keys() {
        let operation = signed_operation();
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 3)
            .unwrap();
        assert_eq!(
            encode(authenticated.operation_hash().as_bytes()),
            "6ec73c02497537d7acc571a76a903455e01b5b134edf22464a66daae04a93fec"
        );
        assert_eq!(
            authenticated.rights_request_hash(),
            operation
                .statement()
                .rights_request()
                .request()
                .request_hash()
                .unwrap()
        );
        assert_eq!(
            authenticated.rights_request_replay_claim_key(),
            operation
                .statement()
                .rights_request()
                .request()
                .replay_claim_key()
                .unwrap()
        );
        assert_eq!(
            authenticated.release_request_replay_claim_key(),
            operation
                .statement()
                .release_request()
                .replay_claim_key()
                .unwrap()
        );
    }

    #[test]
    fn runtime_release_operation_exposes_exact_replay_keys_but_stays_non_actionable() {
        let operation = signed_operation();
        let authenticated = operation
            .verify(operation.statement().runtime_operation_issuer(), NOW + 3)
            .unwrap();
        assert_eq!(
            authenticated.release_request_hash(),
            operation
                .statement()
                .release_request()
                .request_hash()
                .unwrap()
        );
    }

    #[test]
    fn runtime_release_operation_claim_context_rejects_wrong_epoch_and_unknown_node() {
        let wrong_epoch = signed_operation()
            .verify(
                signed_operation().statement().runtime_operation_issuer(),
                NOW + 3,
            )
            .unwrap()
            .validate_node_release_claim_context(
                &envelope_for_epoch(CustodyEpochIdentityV1::new(digest(0xee), 512).unwrap()),
                node_public_key(1),
                NOW + 3,
            );
        assert!(matches!(
            wrong_epoch,
            Err(RuntimeReleaseOperationError::BindingMismatch(
                "key_envelope"
            ))
        ));

        let unknown_node = signed_operation()
            .verify(
                signed_operation().statement().runtime_operation_issuer(),
                NOW + 3,
            )
            .unwrap()
            .validate_node_release_claim_context(
                &envelope_for_epoch(signed_epoch().epoch_identity().unwrap()),
                NodePublicKey::new(
                    SigningKey::from_bytes(&[0x66; 32])
                        .verifying_key()
                        .to_bytes(),
                )
                .unwrap(),
                NOW + 3,
            );
        assert!(matches!(
            unknown_node,
            Err(RuntimeReleaseOperationError::BindingMismatch(
                "custody_node"
            ))
        ));
    }

    #[test]
    fn runtime_release_operation_rejects_invalid_wallet_signature_and_release_hash() {
        let operation = signed_operation();
        let mut wrong_rights = operation.statement().rights_request().clone();
        wrong_rights =
            WalletSignedRightsRequestV1::new(wrong_rights.request().clone(), vec![0; 65]).unwrap();
        let statement = RuntimeReleaseOperationStatementV1::new(
            operation.statement().runtime_operation_issuer(),
            wrong_rights,
            operation.statement().release_request().clone(),
            operation.statement().recipient_public_key(),
            operation.statement().recipient_authorization().clone(),
            operation.statement().policy_body().clone(),
            operation.statement().evidence_request().clone(),
            operation.statement().custody_epoch().clone(),
            operation.statement().audit_request_id(),
            operation.statement().issued_at(),
            operation.statement().expires_at(),
        )
        .unwrap();
        let runtime_key = SigningKey::from_bytes(&[0x42; 32]);
        let wrong = SignedRuntimeReleaseOperationV1::new(
            statement.clone(),
            runtime_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            wrong.verify(wrong.statement().runtime_operation_issuer(), NOW + 3),
            Err(RuntimeReleaseOperationError::Rights(
                RightsError::InvalidWalletSignature
            ))
        );

        let mut wrong_release = operation.statement().release_request().clone();
        wrong_release = KeyReleaseRequestV1::new(
            wrong_release.binding().clone(),
            digest(0xee),
            wrong_release.action(),
            wrong_release.recipient().clone(),
            wrong_release.issued_at(),
            wrong_release.expires_at(),
            wrong_release.replay_nonce(),
        )
        .unwrap();
        assert_eq!(
            RuntimeReleaseOperationStatementV1::new(
                operation.statement().runtime_operation_issuer(),
                operation.statement().rights_request().clone(),
                wrong_release,
                operation.statement().recipient_public_key(),
                operation.statement().recipient_authorization().clone(),
                operation.statement().policy_body().clone(),
                operation.statement().evidence_request().clone(),
                operation.statement().custody_epoch().clone(),
                operation.statement().audit_request_id(),
                operation.statement().issued_at(),
                operation.statement().expires_at(),
            ),
            Err(ContractError::InvalidField(
                "release_request.rights_request_hash"
            ))
        );
    }

    #[test]
    fn runtime_release_operation_rejects_wrong_policy_runtime_issuer_and_binding_mutations() {
        let operation = signed_operation();
        let wrong_policy = RightsPolicyBodyV1::new(
            crate::EncryptedContentIdentityV1::new(digest(0x22), 4096).unwrap(),
            crate::ContentAccessIdV1::new([0x42; 16]).unwrap(),
            RightsActionV1::View,
            RightsSubjectSourceV1::WalletAddress,
            11155111,
            EvmContractAddressV1::new([0x11; 20]).unwrap(),
            EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
            EvmRightsMethodAbiV1::HasAccessByContentIdAddressBytes16,
            RightsObservationFinalityV1::finalized(),
        )
        .unwrap();
        assert_eq!(
            RuntimeReleaseOperationStatementV1::new(
                operation.statement().runtime_operation_issuer(),
                operation.statement().rights_request().clone(),
                operation.statement().release_request().clone(),
                operation.statement().recipient_public_key(),
                operation.statement().recipient_authorization().clone(),
                wrong_policy,
                operation.statement().evidence_request().clone(),
                operation.statement().custody_epoch().clone(),
                operation.statement().audit_request_id(),
                operation.statement().issued_at(),
                operation.statement().expires_at(),
            ),
            Err(ContractError::InvalidField(
                "evidence_request.policy_identity"
            ))
        );

        let wrong_runtime_issuer = RuntimeOperationIssuerKeyV1::new(
            SigningKey::from_bytes(&[0x77; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(
            RuntimeReleaseOperationStatementV1::new(
                wrong_runtime_issuer,
                operation.statement().rights_request().clone(),
                operation.statement().release_request().clone(),
                operation.statement().recipient_public_key(),
                operation.statement().recipient_authorization().clone(),
                operation.statement().policy_body().clone(),
                operation.statement().evidence_request().clone(),
                operation.statement().custody_epoch().clone(),
                operation.statement().audit_request_id(),
                operation.statement().issued_at(),
                operation.statement().expires_at(),
            ),
            Err(ContractError::InvalidField(
                "recipient_authorization.runtime_operation_issuer"
            ))
        );

        let mut wrong_binding = operation
            .statement()
            .rights_request()
            .request()
            .binding()
            .clone();
        wrong_binding = ProtectedContentBindingV1::new(
            wrong_binding.encrypted_content().clone(),
            wrong_binding.key_envelope().clone(),
            wrong_binding.rights_policy().clone(),
            wrong_binding.profile(),
            wrong_binding.wallet(),
            RuntimeSessionBindingV1::new(digest(0xaa)).unwrap(),
        )
        .unwrap();
        let wrong_release = KeyReleaseRequestV1::new(
            wrong_binding.clone(),
            operation
                .statement()
                .release_request()
                .rights_request_hash(),
            operation.statement().release_request().action(),
            operation.statement().release_request().recipient().clone(),
            operation.statement().release_request().issued_at(),
            operation.statement().release_request().expires_at(),
            operation.statement().release_request().replay_nonce(),
        )
        .unwrap();
        assert_eq!(
            RuntimeReleaseOperationStatementV1::new(
                operation.statement().runtime_operation_issuer(),
                operation.statement().rights_request().clone(),
                wrong_release,
                operation.statement().recipient_public_key(),
                operation.statement().recipient_authorization().clone(),
                operation.statement().policy_body().clone(),
                operation.statement().evidence_request().clone(),
                operation.statement().custody_epoch().clone(),
                operation.statement().audit_request_id(),
                operation.statement().issued_at(),
                operation.statement().expires_at(),
            ),
            Err(ContractError::InvalidField("release_request.binding"))
        );
    }

    #[test]
    fn runtime_release_operation_rejects_wrong_recipient_authorization_and_epoch_suite() {
        let operation = signed_operation();
        let wrong_recipient_public_key = crate::test_support::recipient_public_key_bytes(0x21);
        assert_eq!(
            RuntimeReleaseOperationStatementV1::new(
                operation.statement().runtime_operation_issuer(),
                operation.statement().rights_request().clone(),
                operation.statement().release_request().clone(),
                wrong_recipient_public_key,
                operation.statement().recipient_authorization().clone(),
                operation.statement().policy_body().clone(),
                operation.statement().evidence_request().clone(),
                operation.statement().custody_epoch().clone(),
                operation.statement().audit_request_id(),
                operation.statement().issued_at(),
                operation.statement().expires_at(),
            ),
            Err(ContractError::InvalidField(
                "recipient_authorization.recipient_public_key"
            ))
        );

        let mut wrong_epoch = operation.statement().custody_epoch().clone();
        let mut canonical = wrong_epoch.canonical_bytes().unwrap();
        *canonical.last_mut().unwrap() ^= 1;
        wrong_epoch = SignedCustodyEpochV1::from_canonical_bytes(&canonical).unwrap();
        let statement = RuntimeReleaseOperationStatementV1::new(
            operation.statement().runtime_operation_issuer(),
            operation.statement().rights_request().clone(),
            operation.statement().release_request().clone(),
            operation.statement().recipient_public_key(),
            operation.statement().recipient_authorization().clone(),
            operation.statement().policy_body().clone(),
            operation.statement().evidence_request().clone(),
            wrong_epoch,
            operation.statement().audit_request_id(),
            operation.statement().issued_at(),
            operation.statement().expires_at(),
        )
        .unwrap();
        let runtime_key = SigningKey::from_bytes(&[0x42; 32]);
        let wrong = SignedRuntimeReleaseOperationV1::new(
            statement.clone(),
            runtime_key
                .sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            wrong.verify(wrong.statement().runtime_operation_issuer(), NOW + 3),
            Err(RuntimeReleaseOperationError::CustodyEpoch(
                CustodyEpochError::InvalidIssuerSignature
            ))
        );
    }

    #[test]
    fn runtime_release_operation_rejects_wrong_expected_runtime_issuer() {
        let operation = signed_operation();
        let wrong_runtime_issuer = RuntimeOperationIssuerKeyV1::new(
            SigningKey::from_bytes(&[0x77; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap();
        assert_eq!(
            operation.verify(wrong_runtime_issuer, NOW + 3),
            Err(RuntimeReleaseOperationError::BindingMismatch(
                "runtime_operation_issuer"
            ))
        );
    }

    #[test]
    fn runtime_release_operation_decode_rejects_trailing_bytes() {
        let operation = signed_operation();
        let mut trailing = operation.canonical_bytes().unwrap();
        trailing.push(0);
        assert_eq!(
            SignedRuntimeReleaseOperationV1::from_canonical_bytes(&trailing),
            Err(ContractError::TrailingBytes)
        );
    }
}
