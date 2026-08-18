use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, ContractError, Digest32, NodePublicKey,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, SignedNodeRightsDecisionV1,
    SignedRuntimeReleaseOperationV1,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::wire::{
    contract_decode_error, decode_json, encode_json, validate_schema, CanonicalBlob,
    ProviderFailureCodeV1, MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1,
    MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1,
};

pub const RIGHTS_PROVIDER_REQUEST_SCHEMA_V1: &str =
    "elastos.protected-content.rights-provider.request/v1";
pub const RIGHTS_PROVIDER_RESPONSE_SCHEMA_V1: &str =
    "elastos.protected-content.rights-provider.response/v1";

type SignedRuntimeReleaseOperationBlobV1 =
    CanonicalBlob<MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1>;
type SignedNodeRightsDecisionBlobV1 = CanonicalBlob<MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightsProviderRequestOpV1 {
    Evaluate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RightsProviderRequestV1(RightsProviderRequestKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum RightsProviderRequestKindV1 {
    Evaluate {
        schema: String,
        selected_node_public_key: [u8; 32],
        signed_runtime_release_operation: SignedRuntimeReleaseOperationBlobV1,
    },
}

impl RightsProviderRequestV1 {
    pub const fn op(&self) -> RightsProviderRequestOpV1 {
        match self.0 {
            RightsProviderRequestKindV1::Evaluate { .. } => RightsProviderRequestOpV1::Evaluate,
        }
    }

    pub fn new_evaluate(
        selected_node_public_key: NodePublicKey,
        signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
    ) -> Result<Self, ContractError> {
        let value = Self(RightsProviderRequestKindV1::Evaluate {
            schema: RIGHTS_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            selected_node_public_key: *selected_node_public_key.as_bytes(),
            signed_runtime_release_operation: CanonicalBlob::from_contract(
                signed_runtime_release_operation,
            )?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    fn selected_node_public_key(&self) -> Result<NodePublicKey, ContractError> {
        match &self.0 {
            RightsProviderRequestKindV1::Evaluate {
                selected_node_public_key,
                ..
            } => NodePublicKey::new(*selected_node_public_key),
        }
    }

    fn signed_runtime_release_operation(
        &self,
    ) -> Result<SignedRuntimeReleaseOperationV1, ContractError> {
        match &self.0 {
            RightsProviderRequestKindV1::Evaluate {
                signed_runtime_release_operation,
                ..
            } => signed_runtime_release_operation.decode(),
        }
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        match &self.0 {
            RightsProviderRequestKindV1::Evaluate { schema, .. } => {
                validate_schema(
                    schema,
                    RIGHTS_PROVIDER_REQUEST_SCHEMA_V1,
                    "rights_provider_request.schema",
                )?;
                let selected_node = self.selected_node_public_key()?;
                let signed_operation = self.signed_runtime_release_operation()?;
                let node_set = signed_operation
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                if !node_set.contains(selected_node) {
                    return Err(ContractError::InvalidField("selected_node_public_key"));
                }
                Ok(())
            }
        }
    }

    fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let inner = decode_json::<RightsProviderRequestKindV1>(bytes)?;
        let value = Self(inner);
        value.validate_structure().map_err(contract_decode_error)?;
        Ok(value)
    }

    fn into_validated_at(
        self,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now_unix_seconds: u64,
    ) -> Result<ValidatedRightsProviderRequestV1, ContractError> {
        let selected_node_public_key = self.selected_node_public_key()?;
        let signed_runtime_release_operation = self.signed_runtime_release_operation()?;
        let authenticated_runtime_release_operation = signed_runtime_release_operation
            .verify(expected_runtime_issuer, now_unix_seconds)
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
        Ok(ValidatedRightsProviderRequestV1 {
            selected_node_public_key,
            authenticated_runtime_release_operation,
        })
    }
}

impl Serialize for RightsProviderRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

/// Provider-authoritative rights request. Construct only through
/// [`ValidatedRightsProviderRequestV1::decode_and_validate_at`].
///
/// ```compile_fail
/// use elastos_protected_content_provider_contracts::ValidatedRightsProviderRequestV1;
///
/// let _ = serde_json::from_slice::<ValidatedRightsProviderRequestV1>(br#"{}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRightsProviderRequestV1 {
    selected_node_public_key: NodePublicKey,
    authenticated_runtime_release_operation: AuthenticatedRuntimeReleaseOperationV1,
}

impl ValidatedRightsProviderRequestV1 {
    pub fn decode_and_validate_at(
        bytes: &[u8],
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now_unix_seconds: u64,
    ) -> Result<Self, serde_json::Error> {
        RightsProviderRequestV1::decode_wire(bytes)?
            .into_validated_at(expected_runtime_issuer, now_unix_seconds)
            .map_err(contract_decode_error)
    }

    pub const fn op(&self) -> RightsProviderRequestOpV1 {
        RightsProviderRequestOpV1::Evaluate
    }

    pub const fn selected_node_public_key(&self) -> NodePublicKey {
        self.selected_node_public_key
    }

    pub fn authenticated_runtime_release_operation(
        &self,
    ) -> &AuthenticatedRuntimeReleaseOperationV1 {
        &self.authenticated_runtime_release_operation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightsProviderResponseStatusV1 {
    Decision,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RightsProviderResponseV1(RightsProviderResponseKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum RightsProviderResponseKindV1 {
    Decision {
        schema: String,
        signed_node_rights_decision: SignedNodeRightsDecisionBlobV1,
    },
    Failure {
        schema: String,
        audit_request_id: [u8; 32],
        runtime_operation_hash: [u8; 32],
        selected_node_public_key: [u8; 32],
        code: ProviderFailureCodeV1,
    },
}

impl RightsProviderResponseV1 {
    pub const fn status(&self) -> RightsProviderResponseStatusV1 {
        match self.0 {
            RightsProviderResponseKindV1::Decision { .. } => {
                RightsProviderResponseStatusV1::Decision
            }
            RightsProviderResponseKindV1::Failure { .. } => RightsProviderResponseStatusV1::Failure,
        }
    }

    pub fn new_decision(
        signed_node_rights_decision: &SignedNodeRightsDecisionV1,
    ) -> Result<Self, ContractError> {
        let value = Self(RightsProviderResponseKindV1::Decision {
            schema: RIGHTS_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            signed_node_rights_decision: CanonicalBlob::from_contract(signed_node_rights_decision)?,
        });
        value.validate()?;
        Ok(value)
    }

    /// Provider-bound failure. Construct only from a validated provider request.
    ///
    /// ```compile_fail
    /// use elastos_protected_content_provider_contracts::{
    ///     ProviderFailureCodeV1, RightsProviderRequestV1, RightsProviderResponseV1,
    /// };
    ///
    /// # let request: RightsProviderRequestV1 = unimplemented!();
    /// let _ = RightsProviderResponseV1::new_failure(&request, ProviderFailureCodeV1::NotConfigured);
    /// ```
    pub fn new_failure(
        request: &ValidatedRightsProviderRequestV1,
        code: ProviderFailureCodeV1,
    ) -> Result<Self, ContractError> {
        let authenticated = request.authenticated_runtime_release_operation();
        let value = Self(RightsProviderResponseKindV1::Failure {
            schema: RIGHTS_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *authenticated
                .statement()
                .audit_request_id()
                .digest()
                .as_bytes(),
            runtime_operation_hash: *authenticated.operation_hash().as_bytes(),
            selected_node_public_key: *request.selected_node_public_key().as_bytes(),
            code,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn signed_node_rights_decision(&self) -> Result<SignedNodeRightsDecisionV1, ContractError> {
        match &self.0 {
            RightsProviderResponseKindV1::Decision {
                signed_node_rights_decision,
                ..
            } => signed_node_rights_decision.decode(),
            RightsProviderResponseKindV1::Failure { .. } => {
                Err(ContractError::InvalidField("signed_node_rights_decision"))
            }
        }
    }

    pub fn failure_code(&self) -> Result<ProviderFailureCodeV1, ContractError> {
        match self.0 {
            RightsProviderResponseKindV1::Failure { code, .. } => Ok(code),
            RightsProviderResponseKindV1::Decision { .. } => {
                Err(ContractError::InvalidField("provider_failure_code"))
            }
        }
    }

    pub fn failure_audit_request_id(&self) -> Result<RuntimeReleaseAuditIdV1, ContractError> {
        match &self.0 {
            RightsProviderResponseKindV1::Failure {
                audit_request_id, ..
            } => RuntimeReleaseAuditIdV1::new(Digest32::new(*audit_request_id)),
            RightsProviderResponseKindV1::Decision { .. } => {
                Err(ContractError::InvalidField("audit_request_id"))
            }
        }
    }

    pub fn failure_runtime_operation_hash(&self) -> Result<Digest32, ContractError> {
        match &self.0 {
            RightsProviderResponseKindV1::Failure {
                runtime_operation_hash,
                ..
            } => Ok(Digest32::new(*runtime_operation_hash)),
            RightsProviderResponseKindV1::Decision { .. } => {
                Err(ContractError::InvalidField("runtime_operation_hash"))
            }
        }
    }

    pub fn failure_selected_node_public_key(&self) -> Result<NodePublicKey, ContractError> {
        match &self.0 {
            RightsProviderResponseKindV1::Failure {
                selected_node_public_key,
                ..
            } => NodePublicKey::new(*selected_node_public_key),
            RightsProviderResponseKindV1::Decision { .. } => {
                Err(ContractError::InvalidField("selected_node_public_key"))
            }
        }
    }

    pub fn validate_against_request_at(
        &self,
        request: &RightsProviderRequestV1,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now_unix_seconds: u64,
    ) -> Result<(), ContractError> {
        match &self.0 {
            RightsProviderResponseKindV1::Decision {
                signed_node_rights_decision,
                ..
            } => {
                let validated = request
                    .clone()
                    .into_validated_at(expected_runtime_issuer, now_unix_seconds)?;
                let node_set = validated
                    .authenticated_runtime_release_operation()
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let signed_decision: SignedNodeRightsDecisionV1 =
                    signed_node_rights_decision.decode()?;
                if signed_decision.statement().node_public_key()
                    != validated.selected_node_public_key()
                {
                    return Err(ContractError::InvalidField("signed_node_rights_decision"));
                }
                validated
                    .authenticated_runtime_release_operation()
                    .verify_node_rights_decision(&signed_decision, &node_set, now_unix_seconds)
                    .map_err(|_| ContractError::InvalidField("signed_node_rights_decision"))?;
                Ok(())
            }
            RightsProviderResponseKindV1::Failure { .. } => {
                let authenticated = request
                    .clone()
                    .into_validated_at(expected_runtime_issuer, now_unix_seconds)?
                    .authenticated_runtime_release_operation
                    .clone();
                if self.failure_audit_request_id()? != authenticated.statement().audit_request_id()
                {
                    return Err(ContractError::InvalidField("audit_request_id"));
                }
                if self.failure_runtime_operation_hash()? != authenticated.operation_hash() {
                    return Err(ContractError::InvalidField("runtime_operation_hash"));
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
            RightsProviderResponseKindV1::Decision { schema, .. } => {
                validate_schema(
                    schema,
                    RIGHTS_PROVIDER_RESPONSE_SCHEMA_V1,
                    "rights_provider_response.schema",
                )?;
                let _ = self.signed_node_rights_decision()?;
                Ok(())
            }
            RightsProviderResponseKindV1::Failure { schema, .. } => validate_schema(
                schema,
                RIGHTS_PROVIDER_RESPONSE_SCHEMA_V1,
                "rights_provider_response.schema",
            )
            .and_then(|()| {
                let _ = self.failure_audit_request_id()?;
                let _ = self.failure_runtime_operation_hash()?;
                let _ = self.failure_selected_node_public_key()?;
                Ok(())
            }),
        }
    }
}

impl Serialize for RightsProviderResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RightsProviderResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = RightsProviderResponseKindV1::deserialize(deserializer)?;
        let value = Self(inner);
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        test_support::{
            custody_envelope_for_seed, make_signed_node_rights_decision,
            make_signed_runtime_release_operation,
            make_signed_runtime_release_operation_for_envelope_and_seed, node_public_key,
            runtime_operation_issuer_for_seed,
        },
        ProviderFailureCodeV1, RightsProviderRequestV1, RightsProviderResponseStatusV1,
        RightsProviderResponseV1, ValidatedRightsProviderRequestV1,
        RIGHTS_PROVIDER_REQUEST_SCHEMA_V1,
    };

    fn decode_request(bytes: &[u8]) -> Result<ValidatedRightsProviderRequestV1, serde_json::Error> {
        ValidatedRightsProviderRequestV1::decode_and_validate_at(
            bytes,
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
    }

    #[test]
    fn rights_request_round_trips_with_exact_schema() {
        let operation = make_signed_runtime_release_operation();
        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let json = String::from_utf8(request.to_json_vec().unwrap()).unwrap();
        assert!(json.contains(RIGHTS_PROVIDER_REQUEST_SCHEMA_V1));
        let decoded = RightsProviderRequestV1::decode_wire(json.as_bytes()).unwrap();
        assert_eq!(decoded, request);
        let validated = decode_request(json.as_bytes()).unwrap();
        assert_eq!(validated.op(), request.op());
        assert_eq!(validated.selected_node_public_key(), node_public_key(1));
    }

    #[test]
    fn rights_request_rejects_wrong_schema_and_caller_supplied_observed_evidence() {
        let operation = make_signed_runtime_release_operation();
        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&request.to_json_vec().unwrap()).unwrap();
        value["schema"] = serde_json::Value::String("wrong.schema/v1".to_string());
        assert!(decode_request(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut injected =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        injected["observed_evidence"] = serde_json::json!({
            "has_access": true,
            "observed_at": 123,
        });
        assert!(decode_request(&serde_json::to_vec(&injected).unwrap()).is_err());

        let mut injected_has_access =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        injected_has_access["has_access"] = serde_json::json!(true);
        assert!(decode_request(&serde_json::to_vec(&injected_has_access).unwrap()).is_err());

        let mut injected_now =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        injected_now["now_unix_ms"] = serde_json::json!(crate::test_support::NOW + 10);
        assert!(decode_request(&serde_json::to_vec(&injected_now).unwrap()).is_err());
    }

    #[test]
    fn rights_request_rejects_non_member_node_and_invalid_provider_local_time() {
        let operation = make_signed_runtime_release_operation();

        let outsider = node_public_key(0x55);
        assert!(RightsProviderRequestV1::new_evaluate(outsider, &operation).is_err());

        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let json = request.to_json_vec().unwrap();
        assert!(ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &json,
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW.saturating_sub(10),
        )
        .is_err());
        assert!(ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &json,
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 41,
        )
        .is_err());
        assert!(ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &json,
            runtime_operation_issuer_for_seed(0x43),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn rights_response_round_trips_and_rejects_request_substitution() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(
            &operation,
            1,
            elastos_protected_content_contracts::RightsDecisionV1::Allowed,
        );
        let response = RightsProviderResponseV1::new_decision(&decision).unwrap();
        assert_eq!(response.status(), RightsProviderResponseStatusV1::Decision);
        let decoded =
            RightsProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded.signed_node_rights_decision().unwrap(), decision);
        decoded
            .validate_against_request_at(
                &RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap(),
                runtime_operation_issuer_for_seed(0x42),
                crate::test_support::NOW + 10,
            )
            .unwrap();

        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        assert!(
            RightsProviderResponseV1::from_json_slice(&request.to_json_vec().unwrap()).is_err()
        );
    }

    #[test]
    fn rights_response_failure_is_typed_and_debug_is_redacted() {
        let operation = make_signed_runtime_release_operation();
        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let validated = decode_request(&request.to_json_vec().unwrap()).unwrap();
        let failure =
            RightsProviderResponseV1::new_failure(&validated, ProviderFailureCodeV1::NotConfigured)
                .unwrap();
        assert_eq!(failure.status(), RightsProviderResponseStatusV1::Failure);
        assert_eq!(
            failure.failure_code().unwrap(),
            ProviderFailureCodeV1::NotConfigured
        );
        assert_eq!(
            failure.failure_audit_request_id().unwrap(),
            validated
                .authenticated_runtime_release_operation()
                .statement()
                .audit_request_id()
        );
        assert_eq!(
            failure.failure_runtime_operation_hash().unwrap(),
            validated
                .authenticated_runtime_release_operation()
                .operation_hash()
        );
        assert_eq!(
            failure.failure_selected_node_public_key().unwrap(),
            validated.selected_node_public_key()
        );
        failure
            .validate_against_request_at(
                &request,
                runtime_operation_issuer_for_seed(0x42),
                crate::test_support::NOW + 10,
            )
            .unwrap();

        let decision = make_signed_node_rights_decision(
            &operation,
            1,
            elastos_protected_content_contracts::RightsDecisionV1::Allowed,
        );
        let response = RightsProviderResponseV1::new_decision(&decision).unwrap();
        let debug = format!("{response:?}");
        assert!(!debug.contains(&format!("{:?}", decision.node_signature())));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn rights_response_failure_rejects_replayed_or_delayed_mismatch() {
        let operation = make_signed_runtime_release_operation();
        let other_operation = make_signed_runtime_release_operation_for_envelope_and_seed(
            0x44,
            &custody_envelope_for_seed(0x44),
        );
        let request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &operation).unwrap();
        let other_request =
            RightsProviderRequestV1::new_evaluate(node_public_key(1), &other_operation).unwrap();
        let validated_other = ValidatedRightsProviderRequestV1::decode_and_validate_at(
            &other_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x44),
            crate::test_support::NOW + 10,
        )
        .unwrap();
        let failure = RightsProviderResponseV1::new_failure(
            &validated_other,
            ProviderFailureCodeV1::BackendUnavailable,
        )
        .unwrap();
        assert!(failure
            .validate_against_request_at(
                &request,
                runtime_operation_issuer_for_seed(0x42),
                crate::test_support::NOW + 10,
            )
            .is_err());
    }
}
