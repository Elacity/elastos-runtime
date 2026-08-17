use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, ContractError, CustodyEnvelopeV1, Digest32,
    NodePublicKey, RuntimeReleaseAuditIdV1, SignedNodeContributionV1, SignedNodeRightsDecisionV1,
    SignedRuntimeReleaseOperationV1,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::wire::{
    contract_decode_error, decode_json, encode_json, validate_schema, CanonicalBlob,
    ProviderFailureCodeV1, MAX_CUSTODY_ENVELOPE_BYTES_V1, MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1,
    MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1, MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1,
};

pub const CUSTODY_PROVIDER_REQUEST_SCHEMA_V1: &str =
    "elastos.protected-content.custody-provider.request/v1";
pub const CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1: &str =
    "elastos.protected-content.custody-provider.response/v1";

type SignedRuntimeReleaseOperationBlobV1 =
    CanonicalBlob<MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1>;
type SignedNodeRightsDecisionBlobV1 = CanonicalBlob<MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1>;
type CustodyEnvelopeBlobV1 = CanonicalBlob<MAX_CUSTODY_ENVELOPE_BYTES_V1>;
type SignedNodeContributionBlobV1 = CanonicalBlob<MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyProviderRequestOpV1 {
    ReleaseContribution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyProviderRequestV1(CustodyProviderRequestKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum CustodyProviderRequestKindV1 {
    ReleaseContribution {
        schema: String,
        selected_node_public_key: [u8; 32],
        signed_runtime_release_operation: SignedRuntimeReleaseOperationBlobV1,
        signed_node_rights_decision: SignedNodeRightsDecisionBlobV1,
        custody_envelope: CustodyEnvelopeBlobV1,
    },
}

impl CustodyProviderRequestV1 {
    pub const fn op(&self) -> CustodyProviderRequestOpV1 {
        match self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution { .. } => {
                CustodyProviderRequestOpV1::ReleaseContribution
            }
        }
    }

    pub fn new_release_contribution(
        selected_node_public_key: NodePublicKey,
        signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
        signed_node_rights_decision: &SignedNodeRightsDecisionV1,
        custody_envelope: &CustodyEnvelopeV1,
    ) -> Result<Self, ContractError> {
        let value = Self(CustodyProviderRequestKindV1::ReleaseContribution {
            schema: CUSTODY_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            selected_node_public_key: *selected_node_public_key.as_bytes(),
            signed_runtime_release_operation: CanonicalBlob::from_contract(
                signed_runtime_release_operation,
            )?,
            signed_node_rights_decision: CanonicalBlob::from_contract(signed_node_rights_decision)?,
            custody_envelope: CanonicalBlob::from_contract(custody_envelope)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    fn selected_node_public_key(&self) -> Result<NodePublicKey, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution {
                selected_node_public_key,
                ..
            } => NodePublicKey::new(*selected_node_public_key),
        }
    }

    fn signed_runtime_release_operation(
        &self,
    ) -> Result<SignedRuntimeReleaseOperationV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution {
                signed_runtime_release_operation,
                ..
            } => signed_runtime_release_operation.decode(),
        }
    }

    fn signed_node_rights_decision(&self) -> Result<SignedNodeRightsDecisionV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution {
                signed_node_rights_decision,
                ..
            } => signed_node_rights_decision.decode(),
        }
    }

    fn custody_envelope(&self) -> Result<CustodyEnvelopeV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution {
                custody_envelope, ..
            } => custody_envelope.decode(),
        }
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution { schema, .. } => {
                validate_schema(
                    schema,
                    CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
                    "custody_provider_request.schema",
                )?;
                let selected_node = self.selected_node_public_key()?;
                let signed_operation = self.signed_runtime_release_operation()?;
                let signed_decision = self.signed_node_rights_decision()?;
                let envelope = self.custody_envelope()?;
                signed_operation
                    .verify(signed_operation.statement().issued_at())
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let node_set = signed_operation
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                if !node_set.contains(selected_node) {
                    return Err(ContractError::InvalidField("selected_node_public_key"));
                }
                if signed_decision.statement().node_public_key() != selected_node {
                    return Err(ContractError::InvalidField("signed_node_rights_decision"));
                }
                if !envelope.matches_key_envelope_identity(
                    signed_operation
                        .statement()
                        .release_request()
                        .binding()
                        .key_envelope(),
                )? {
                    return Err(ContractError::InvalidField("custody_envelope"));
                }
                Ok(())
            }
        }
    }

    fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let inner = decode_json::<CustodyProviderRequestKindV1>(bytes)?;
        let value = Self(inner);
        value.validate_structure().map_err(contract_decode_error)?;
        Ok(value)
    }

    fn into_validated_at(
        self,
        now_unix_ms: u64,
    ) -> Result<ValidatedCustodyProviderRequestV1, ContractError> {
        let selected_node_public_key = self.selected_node_public_key()?;
        let signed_node_rights_decision = self.signed_node_rights_decision()?;
        let custody_envelope = self.custody_envelope()?;
        let authenticated_runtime_release_operation = self
            .signed_runtime_release_operation()?
            .verify(now_unix_ms)
            .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
        let node_set = authenticated_runtime_release_operation
            .statement()
            .custody_epoch()
            .statement()
            .node_set()
            .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
        if !node_set.contains(selected_node_public_key) {
            return Err(ContractError::InvalidField("selected_node_public_key"));
        }
        if signed_node_rights_decision.statement().node_public_key() != selected_node_public_key {
            return Err(ContractError::InvalidField("signed_node_rights_decision"));
        }
        authenticated_runtime_release_operation
            .verify_node_rights_decision(&signed_node_rights_decision, &node_set, now_unix_ms)
            .map_err(|_| ContractError::InvalidField("signed_node_rights_decision"))?;
        authenticated_runtime_release_operation
            .validate_node_release_claim_context(
                &custody_envelope,
                selected_node_public_key,
                now_unix_ms,
            )
            .map_err(|_| ContractError::InvalidField("custody_envelope"))?;
        Ok(ValidatedCustodyProviderRequestV1 {
            selected_node_public_key,
            authenticated_runtime_release_operation,
            signed_node_rights_decision,
            custody_envelope,
        })
    }
}

impl Serialize for CustodyProviderRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

/// Provider-authoritative custody request. Construct only through
/// [`ValidatedCustodyProviderRequestV1::decode_and_validate_at`].
///
/// ```compile_fail
/// use elastos_protected_content_provider_contracts::ValidatedCustodyProviderRequestV1;
///
/// let _ = serde_json::from_slice::<ValidatedCustodyProviderRequestV1>(br#"{}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCustodyProviderRequestV1 {
    selected_node_public_key: NodePublicKey,
    authenticated_runtime_release_operation: AuthenticatedRuntimeReleaseOperationV1,
    signed_node_rights_decision: SignedNodeRightsDecisionV1,
    custody_envelope: CustodyEnvelopeV1,
}

impl ValidatedCustodyProviderRequestV1 {
    pub fn decode_and_validate_at(
        bytes: &[u8],
        now_unix_ms: u64,
    ) -> Result<Self, serde_json::Error> {
        CustodyProviderRequestV1::decode_wire(bytes)?
            .into_validated_at(now_unix_ms)
            .map_err(contract_decode_error)
    }

    pub const fn op(&self) -> CustodyProviderRequestOpV1 {
        CustodyProviderRequestOpV1::ReleaseContribution
    }

    pub const fn selected_node_public_key(&self) -> NodePublicKey {
        self.selected_node_public_key
    }

    pub fn authenticated_runtime_release_operation(
        &self,
    ) -> &AuthenticatedRuntimeReleaseOperationV1 {
        &self.authenticated_runtime_release_operation
    }

    pub fn signed_node_rights_decision(&self) -> &SignedNodeRightsDecisionV1 {
        &self.signed_node_rights_decision
    }

    pub fn custody_envelope(&self) -> &CustodyEnvelopeV1 {
        &self.custody_envelope
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyProviderResponseStatusV1 {
    Contribution,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyProviderResponseV1(CustodyProviderResponseKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum CustodyProviderResponseKindV1 {
    Contribution {
        schema: String,
        signed_node_contribution: SignedNodeContributionBlobV1,
    },
    Failure {
        schema: String,
        audit_request_id: [u8; 32],
        release_request_hash: [u8; 32],
        selected_node_public_key: [u8; 32],
        code: ProviderFailureCodeV1,
    },
}

impl CustodyProviderResponseV1 {
    pub const fn status(&self) -> CustodyProviderResponseStatusV1 {
        match self.0 {
            CustodyProviderResponseKindV1::Contribution { .. } => {
                CustodyProviderResponseStatusV1::Contribution
            }
            CustodyProviderResponseKindV1::Failure { .. } => {
                CustodyProviderResponseStatusV1::Failure
            }
        }
    }

    pub fn new_contribution(
        signed_node_contribution: &SignedNodeContributionV1,
    ) -> Result<Self, ContractError> {
        let value = Self(CustodyProviderResponseKindV1::Contribution {
            schema: CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            signed_node_contribution: CanonicalBlob::from_contract(signed_node_contribution)?,
        });
        value.validate()?;
        Ok(value)
    }

    /// Provider-bound failure. Construct only from a validated provider request.
    ///
    /// ```compile_fail
    /// use elastos_protected_content_provider_contracts::{
    ///     CustodyProviderRequestV1, CustodyProviderResponseV1, ProviderFailureCodeV1,
    /// };
    ///
    /// # let request: CustodyProviderRequestV1 = unimplemented!();
    /// let _ = CustodyProviderResponseV1::new_failure(&request, ProviderFailureCodeV1::NotConfigured);
    /// ```
    pub fn new_failure(
        request: &ValidatedCustodyProviderRequestV1,
        code: ProviderFailureCodeV1,
    ) -> Result<Self, ContractError> {
        let authenticated = request.authenticated_runtime_release_operation();
        let value = Self(CustodyProviderResponseKindV1::Failure {
            schema: CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *authenticated
                .statement()
                .audit_request_id()
                .digest()
                .as_bytes(),
            release_request_hash: *authenticated.release_request_hash().as_bytes(),
            selected_node_public_key: *request.selected_node_public_key().as_bytes(),
            code,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn signed_node_contribution(&self) -> Result<SignedNodeContributionV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Contribution {
                signed_node_contribution,
                ..
            } => signed_node_contribution.decode(),
            CustodyProviderResponseKindV1::Failure { .. } => {
                Err(ContractError::InvalidField("signed_node_contribution"))
            }
        }
    }

    pub fn failure_code(&self) -> Result<ProviderFailureCodeV1, ContractError> {
        match self.0 {
            CustodyProviderResponseKindV1::Failure { code, .. } => Ok(code),
            CustodyProviderResponseKindV1::Contribution { .. } => {
                Err(ContractError::InvalidField("provider_failure_code"))
            }
        }
    }

    pub fn failure_audit_request_id(&self) -> Result<RuntimeReleaseAuditIdV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Failure {
                audit_request_id, ..
            } => RuntimeReleaseAuditIdV1::new(Digest32::new(*audit_request_id)),
            CustodyProviderResponseKindV1::Contribution { .. } => {
                Err(ContractError::InvalidField("audit_request_id"))
            }
        }
    }

    pub fn failure_release_request_hash(&self) -> Result<Digest32, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Failure {
                release_request_hash,
                ..
            } => Ok(Digest32::new(*release_request_hash)),
            CustodyProviderResponseKindV1::Contribution { .. } => {
                Err(ContractError::InvalidField("release_request_hash"))
            }
        }
    }

    pub fn failure_selected_node_public_key(&self) -> Result<NodePublicKey, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Failure {
                selected_node_public_key,
                ..
            } => NodePublicKey::new(*selected_node_public_key),
            CustodyProviderResponseKindV1::Contribution { .. } => {
                Err(ContractError::InvalidField("selected_node_public_key"))
            }
        }
    }

    pub fn validate_against_request_at(
        &self,
        request: &CustodyProviderRequestV1,
        now_unix_ms: u64,
    ) -> Result<(), ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Contribution {
                signed_node_contribution,
                ..
            } => {
                let validated = request.clone().into_validated_at(now_unix_ms)?;
                let node_set = validated
                    .authenticated_runtime_release_operation()
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let contribution: SignedNodeContributionV1 = signed_node_contribution.decode()?;
                let verified = validated
                    .authenticated_runtime_release_operation()
                    .verify_node_contribution(&contribution, &node_set, now_unix_ms)
                    .map_err(|_| ContractError::InvalidField("signed_node_contribution"))?;
                if verified.node_public_key() != validated.selected_node_public_key() {
                    return Err(ContractError::InvalidField("signed_node_contribution"));
                }
                Ok(())
            }
            CustodyProviderResponseKindV1::Failure { .. } => {
                let signed_operation = request.signed_runtime_release_operation()?;
                let authenticated = signed_operation
                    .verify(signed_operation.statement().issued_at())
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                if self.failure_audit_request_id()? != authenticated.statement().audit_request_id()
                {
                    return Err(ContractError::InvalidField("audit_request_id"));
                }
                if self.failure_release_request_hash()? != authenticated.release_request_hash() {
                    return Err(ContractError::InvalidField("release_request_hash"));
                }
                if self.failure_selected_node_public_key()? != request.selected_node_public_key()? {
                    return Err(ContractError::InvalidField("selected_node_public_key"));
                }
                Ok(())
            }
        }
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        decode_json(bytes)
    }

    fn validate(&self) -> Result<(), ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Contribution { schema, .. } => {
                validate_schema(
                    schema,
                    CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
                    "custody_provider_response.schema",
                )?;
                let _ = self.signed_node_contribution()?;
                Ok(())
            }
            CustodyProviderResponseKindV1::Failure { schema, .. } => validate_schema(
                schema,
                CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
                "custody_provider_response.schema",
            )
            .and_then(|()| {
                let _ = self.failure_audit_request_id()?;
                let _ = self.failure_release_request_hash()?;
                let _ = self.failure_selected_node_public_key()?;
                Ok(())
            }),
        }
    }
}

impl Serialize for CustodyProviderResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CustodyProviderResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = CustodyProviderResponseKindV1::deserialize(deserializer)?;
        let value = Self(inner);
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use elastos_protected_content_contracts::{
        CustodyEnvelopeManifestV1, CustodyEnvelopeV1, EncryptedContentIdentityV1, HpkeCiphertextV1,
        NodePublicKey, RightsDecisionV1, ThresholdV1, HPKE_ENCAPPED_KEY_BYTES,
        HPKE_SEALED_SHARE_BYTES,
    };

    use crate::{
        test_support::{
            custody_envelope, custody_envelope_for_seed, digest, make_signed_node_contribution,
            make_signed_node_rights_decision, make_signed_runtime_release_operation,
            make_signed_runtime_release_operation_for_envelope_and_seed, node_public_key,
            signed_custody_epoch,
        },
        CustodyProviderRequestV1, CustodyProviderResponseStatusV1, CustodyProviderResponseV1,
        ProviderFailureCodeV1, ValidatedCustodyProviderRequestV1,
    };

    #[test]
    fn custody_request_round_trips_with_exact_schema() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let request = CustodyProviderRequestV1::new_release_contribution(
            node_public_key(1),
            &operation,
            &decision,
            &custody_envelope(),
        )
        .unwrap();
        let decoded =
            CustodyProviderRequestV1::decode_wire(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, request);
        let validated = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            crate::test_support::NOW + 10,
        )
        .unwrap();
        assert_eq!(validated.op(), request.op());
        assert_eq!(validated.selected_node_public_key(), node_public_key(1));

        let mut injected_now =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        injected_now["now_unix_ms"] = serde_json::json!(crate::test_support::NOW + 10);
        assert!(ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &serde_json::to_vec(&injected_now).unwrap(),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn custody_request_rejects_wrong_selected_node_wrong_envelope_and_wrong_decision_binding() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let wrong_node = NodePublicKey::new(
            SigningKey::from_bytes(&[0x55; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap();
        assert!(CustodyProviderRequestV1::new_release_contribution(
            wrong_node,
            &operation,
            &decision,
            &custody_envelope(),
        )
        .is_err());

        let other_operation = make_signed_runtime_release_operation_for_envelope_and_seed(
            0x52,
            &custody_envelope_for_seed(0x52),
        );
        let epoch = signed_custody_epoch();
        let shares = [0x71u8, 0x72, 0x73]
            .into_iter()
            .map(|seed| {
                let mut encapped_key = [0u8; HPKE_ENCAPPED_KEY_BYTES];
                encapped_key[0] = seed;
                let mut ciphertext = [0u8; HPKE_SEALED_SHARE_BYTES];
                ciphertext.fill(seed);
                HpkeCiphertextV1::new(encapped_key, ciphertext).unwrap()
            })
            .collect();
        let wrong_envelope = CustodyEnvelopeV1::new(
            CustodyEnvelopeManifestV1::new(
                EncryptedContentIdentityV1::new(digest(0x44), 4096).unwrap(),
                epoch.epoch_identity().unwrap(),
                ThresholdV1::new(2, 3).unwrap(),
                digest(0x45),
                epoch.statement().nodes().to_vec(),
            )
            .unwrap(),
            shares,
        )
        .unwrap();
        assert!(CustodyProviderRequestV1::new_release_contribution(
            node_public_key(1),
            &operation,
            &decision,
            &wrong_envelope,
        )
        .is_err());
        let wrong_decision_request = CustodyProviderRequestV1::new_release_contribution(
            node_public_key(1),
            &operation,
            &make_signed_node_rights_decision(&other_operation, 1, RightsDecisionV1::Allowed),
            &custody_envelope(),
        )
        .unwrap();
        assert!(ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &wrong_decision_request.to_json_vec().unwrap(),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn custody_response_round_trips_and_rejects_request_substitution() {
        let operation = make_signed_runtime_release_operation();
        let contribution = make_signed_node_contribution(&operation, 1);
        let response = CustodyProviderResponseV1::new_contribution(&contribution).unwrap();
        assert_eq!(
            response.status(),
            CustodyProviderResponseStatusV1::Contribution
        );
        let decoded =
            CustodyProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded.signed_node_contribution().unwrap(), contribution);

        let request = {
            let decision =
                make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
            CustodyProviderRequestV1::new_release_contribution(
                node_public_key(1),
                &operation,
                &decision,
                &custody_envelope(),
            )
            .unwrap()
        };
        decoded
            .validate_against_request_at(&request, crate::test_support::NOW + 10)
            .unwrap();
        assert!(
            CustodyProviderResponseV1::from_json_slice(&request.to_json_vec().unwrap()).is_err()
        );
    }

    #[test]
    fn custody_response_failure_is_typed() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let request = CustodyProviderRequestV1::new_release_contribution(
            node_public_key(1),
            &operation,
            &decision,
            &custody_envelope(),
        )
        .unwrap();
        let validated = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            crate::test_support::NOW + 10,
        )
        .unwrap();
        let response = CustodyProviderResponseV1::new_failure(
            &validated,
            ProviderFailureCodeV1::NotConfigured,
        )
        .unwrap();
        assert_eq!(response.status(), CustodyProviderResponseStatusV1::Failure);
        assert_eq!(
            response.failure_code().unwrap(),
            ProviderFailureCodeV1::NotConfigured
        );
        assert_eq!(
            response.failure_audit_request_id().unwrap(),
            validated
                .authenticated_runtime_release_operation()
                .statement()
                .audit_request_id()
        );
        assert_eq!(
            response.failure_release_request_hash().unwrap(),
            validated
                .authenticated_runtime_release_operation()
                .release_request_hash()
        );
        assert_eq!(
            response.failure_selected_node_public_key().unwrap(),
            validated.selected_node_public_key()
        );
        response
            .validate_against_request_at(&request, crate::test_support::NOW + 10)
            .unwrap();
    }

    #[test]
    fn custody_response_failure_rejects_delayed_or_replayed_mismatch() {
        let operation = make_signed_runtime_release_operation();
        let other_operation = make_signed_runtime_release_operation_for_envelope_and_seed(
            0x44,
            &custody_envelope_for_seed(0x44),
        );
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let other_decision =
            make_signed_node_rights_decision(&other_operation, 1, RightsDecisionV1::Allowed);
        let request = CustodyProviderRequestV1::new_release_contribution(
            node_public_key(1),
            &operation,
            &decision,
            &custody_envelope(),
        )
        .unwrap();
        let other_request = CustodyProviderRequestV1::new_release_contribution(
            node_public_key(1),
            &other_operation,
            &other_decision,
            &custody_envelope_for_seed(0x44),
        )
        .unwrap();
        let validated_other = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &other_request.to_json_vec().unwrap(),
            crate::test_support::NOW + 10,
        )
        .unwrap();
        let response = CustodyProviderResponseV1::new_failure(
            &validated_other,
            ProviderFailureCodeV1::BackendUnavailable,
        )
        .unwrap();
        assert!(response
            .validate_against_request_at(&request, crate::test_support::NOW + 10)
            .is_err());
    }
}
