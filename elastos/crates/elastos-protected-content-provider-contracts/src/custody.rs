use elastos_protected_content_contracts::{
    AuthenticatedRuntimeCustodyProvisioningV1, AuthenticatedRuntimeReleaseOperationV1,
    CanonicalContract, ContractError, CustodyNodeProvisioningRecordIdentityV1,
    CustodyNodeProvisioningRecordV1, Digest32, NodePublicKey, RuntimeCustodyProvisioningIdV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, SignedNodeContributionV1,
    SignedNodeRightsDecisionV1, SignedRuntimeCustodyProvisioningV1,
    SignedRuntimeReleaseOperationV1,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::wire::{
    contract_decode_error, decode_json, encode_json, validate_schema, CanonicalBlob,
    ProviderFailureCodeV1, MAX_CUSTODY_NODE_PROVISIONING_RECORD_BYTES_V1,
    MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1, MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1,
    MAX_SIGNED_RUNTIME_CUSTODY_PROVISIONING_BYTES_V1,
    MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1,
};

pub const CUSTODY_PROVIDER_REQUEST_SCHEMA_V1: &str =
    "elastos.protected-content.custody-provider.request/v1";
pub const CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1: &str =
    "elastos.protected-content.custody-provider.response/v1";

type SignedRuntimeReleaseOperationBlobV1 =
    CanonicalBlob<MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1>;
type SignedRuntimeCustodyProvisioningBlobV1 =
    CanonicalBlob<MAX_SIGNED_RUNTIME_CUSTODY_PROVISIONING_BYTES_V1>;
type SignedNodeRightsDecisionBlobV1 = CanonicalBlob<MAX_SIGNED_NODE_RIGHTS_DECISION_BYTES_V1>;
type CustodyNodeProvisioningRecordBlobV1 =
    CanonicalBlob<MAX_CUSTODY_NODE_PROVISIONING_RECORD_BYTES_V1>;
type SignedNodeContributionBlobV1 = CanonicalBlob<MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyProviderRequestOpV1 {
    ProvisionNodeShare,
    ReleaseContribution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyProviderRequestV1(CustodyProviderRequestKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum CustodyProviderRequestKindV1 {
    ProvisionNodeShare {
        schema: String,
        custody_node_provisioning_record: CustodyNodeProvisioningRecordBlobV1,
        signed_runtime_custody_provisioning: SignedRuntimeCustodyProvisioningBlobV1,
    },
    ReleaseContribution {
        schema: String,
        signed_runtime_release_operation: SignedRuntimeReleaseOperationBlobV1,
        signed_node_rights_decision: SignedNodeRightsDecisionBlobV1,
    },
}

impl CustodyProviderRequestV1 {
    pub fn op(&self) -> CustodyProviderRequestOpV1 {
        match &self.0 {
            CustodyProviderRequestKindV1::ProvisionNodeShare { .. } => {
                CustodyProviderRequestOpV1::ProvisionNodeShare
            }
            CustodyProviderRequestKindV1::ReleaseContribution { .. } => {
                CustodyProviderRequestOpV1::ReleaseContribution
            }
        }
    }

    pub fn new_provision_node_share(
        custody_node_provisioning_record: &CustodyNodeProvisioningRecordV1,
        signed_runtime_custody_provisioning: &SignedRuntimeCustodyProvisioningV1,
    ) -> Result<Self, ContractError> {
        let value = Self(CustodyProviderRequestKindV1::ProvisionNodeShare {
            schema: CUSTODY_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            custody_node_provisioning_record: CanonicalBlob::from_contract(
                custody_node_provisioning_record,
            )?,
            signed_runtime_custody_provisioning: CanonicalBlob::from_contract(
                signed_runtime_custody_provisioning,
            )?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_release_contribution(
        signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
        signed_node_rights_decision: &SignedNodeRightsDecisionV1,
    ) -> Result<Self, ContractError> {
        let value = Self(CustodyProviderRequestKindV1::ReleaseContribution {
            schema: CUSTODY_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            signed_runtime_release_operation: CanonicalBlob::from_contract(
                signed_runtime_release_operation,
            )?,
            signed_node_rights_decision: CanonicalBlob::from_contract(signed_node_rights_decision)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    fn custody_node_provisioning_record(
        &self,
    ) -> Result<CustodyNodeProvisioningRecordV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ProvisionNodeShare {
                custody_node_provisioning_record,
                ..
            } => custody_node_provisioning_record.decode(),
            CustodyProviderRequestKindV1::ReleaseContribution { .. } => Err(
                ContractError::InvalidField("custody_node_provisioning_record"),
            ),
        }
    }

    fn signed_runtime_custody_provisioning(
        &self,
    ) -> Result<SignedRuntimeCustodyProvisioningV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ProvisionNodeShare {
                signed_runtime_custody_provisioning,
                ..
            } => signed_runtime_custody_provisioning.decode(),
            CustodyProviderRequestKindV1::ReleaseContribution { .. } => Err(
                ContractError::InvalidField("signed_runtime_custody_provisioning"),
            ),
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
            CustodyProviderRequestKindV1::ProvisionNodeShare { .. } => Err(
                ContractError::InvalidField("signed_runtime_release_operation"),
            ),
        }
    }

    fn signed_node_rights_decision(&self) -> Result<SignedNodeRightsDecisionV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ReleaseContribution {
                signed_node_rights_decision,
                ..
            } => signed_node_rights_decision.decode(),
            CustodyProviderRequestKindV1::ProvisionNodeShare { .. } => {
                Err(ContractError::InvalidField("signed_node_rights_decision"))
            }
        }
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ProvisionNodeShare {
                schema,
                custody_node_provisioning_record,
                ..
            } => {
                validate_schema(
                    schema,
                    CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
                    "custody_provider_request.schema",
                )?;
                let record = self.custody_node_provisioning_record()?;
                if record.canonical_bytes()? != custody_node_provisioning_record.as_slice() {
                    return Err(ContractError::InvalidField(
                        "custody_node_provisioning_record",
                    ));
                }
                let signed_provisioning = self.signed_runtime_custody_provisioning()?;
                if signed_provisioning.statement().record_identity() != record.record_identity()? {
                    return Err(ContractError::InvalidField(
                        "signed_runtime_custody_provisioning",
                    ));
                }
                Ok(())
            }
            CustodyProviderRequestKindV1::ReleaseContribution { schema, .. } => {
                validate_schema(
                    schema,
                    CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
                    "custody_provider_request.schema",
                )?;
                let signed_operation = self.signed_runtime_release_operation()?;
                let signed_decision = self.signed_node_rights_decision()?;
                let selected_node = signed_decision.statement().node_public_key();
                let node_set = signed_operation
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                if !node_set.contains(selected_node) {
                    return Err(ContractError::InvalidField("signed_node_rights_decision"));
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
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        expected_local_node_public_key: NodePublicKey,
        now_unix_ms: u64,
    ) -> Result<ValidatedCustodyProviderRequestV1, ContractError> {
        match &self.0 {
            CustodyProviderRequestKindV1::ProvisionNodeShare { .. } => {
                let record = self.custody_node_provisioning_record()?;
                let signed_provisioning = self.signed_runtime_custody_provisioning()?;
                let authenticated_runtime_custody_provisioning = signed_provisioning
                    .verify_for_record(&record, expected_runtime_issuer, now_unix_ms)
                    .map_err(|_| {
                        ContractError::InvalidField("signed_runtime_custody_provisioning")
                    })?;
                if record.selected_node_public_key() != expected_local_node_public_key {
                    return Err(ContractError::InvalidField("selected_node_public_key"));
                }
                Ok(ValidatedCustodyProviderRequestV1 {
                    kind: ValidatedCustodyProviderRequestKindV1::ProvisionNodeShare(Box::new(
                        ValidatedCustodyProvisionNodeShareRequestV1 {
                            selected_node_public_key: expected_local_node_public_key,
                            custody_node_provisioning_record: record,
                            authenticated_runtime_custody_provisioning,
                        },
                    )),
                })
            }
            CustodyProviderRequestKindV1::ReleaseContribution { .. } => {
                let signed_node_rights_decision = self.signed_node_rights_decision()?;
                let authenticated_runtime_release_operation = self
                    .signed_runtime_release_operation()?
                    .verify(expected_runtime_issuer, now_unix_ms)
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let node_set = authenticated_runtime_release_operation
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let selected_node_public_key =
                    signed_node_rights_decision.statement().node_public_key();
                if selected_node_public_key != expected_local_node_public_key {
                    return Err(ContractError::InvalidField("selected_node_public_key"));
                }
                if !node_set.contains(selected_node_public_key) {
                    return Err(ContractError::InvalidField("signed_node_rights_decision"));
                }
                authenticated_runtime_release_operation
                    .verify_node_rights_decision(
                        &signed_node_rights_decision,
                        &node_set,
                        now_unix_ms,
                    )
                    .map_err(|_| ContractError::InvalidField("signed_node_rights_decision"))?;
                Ok(ValidatedCustodyProviderRequestV1 {
                    kind: ValidatedCustodyProviderRequestKindV1::ReleaseContribution(Box::new(
                        ValidatedCustodyReleaseContributionRequestV1 {
                            selected_node_public_key,
                            authenticated_runtime_release_operation,
                            signed_node_rights_decision,
                        },
                    )),
                })
            }
        }
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
    kind: ValidatedCustodyProviderRequestKindV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValidatedCustodyProviderRequestKindV1 {
    ProvisionNodeShare(Box<ValidatedCustodyProvisionNodeShareRequestV1>),
    ReleaseContribution(Box<ValidatedCustodyReleaseContributionRequestV1>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCustodyProvisionNodeShareRequestV1 {
    selected_node_public_key: NodePublicKey,
    custody_node_provisioning_record: CustodyNodeProvisioningRecordV1,
    authenticated_runtime_custody_provisioning: AuthenticatedRuntimeCustodyProvisioningV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCustodyReleaseContributionRequestV1 {
    selected_node_public_key: NodePublicKey,
    authenticated_runtime_release_operation: AuthenticatedRuntimeReleaseOperationV1,
    signed_node_rights_decision: SignedNodeRightsDecisionV1,
}

impl ValidatedCustodyProviderRequestV1 {
    pub fn decode_and_validate_at(
        bytes: &[u8],
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        expected_local_node_public_key: NodePublicKey,
        now_unix_ms: u64,
    ) -> Result<Self, serde_json::Error> {
        CustodyProviderRequestV1::decode_wire(bytes)?
            .into_validated_at(
                expected_runtime_issuer,
                expected_local_node_public_key,
                now_unix_ms,
            )
            .map_err(contract_decode_error)
    }

    pub fn op(&self) -> CustodyProviderRequestOpV1 {
        match &self.kind {
            ValidatedCustodyProviderRequestKindV1::ProvisionNodeShare(_) => {
                CustodyProviderRequestOpV1::ProvisionNodeShare
            }
            ValidatedCustodyProviderRequestKindV1::ReleaseContribution(_) => {
                CustodyProviderRequestOpV1::ReleaseContribution
            }
        }
    }

    pub fn provision_node_share(
        &self,
    ) -> Result<&ValidatedCustodyProvisionNodeShareRequestV1, ContractError> {
        match &self.kind {
            ValidatedCustodyProviderRequestKindV1::ProvisionNodeShare(request) => Ok(request),
            ValidatedCustodyProviderRequestKindV1::ReleaseContribution(_) => {
                Err(ContractError::InvalidField("provision_node_share"))
            }
        }
    }

    pub fn release_contribution(
        &self,
    ) -> Result<&ValidatedCustodyReleaseContributionRequestV1, ContractError> {
        match &self.kind {
            ValidatedCustodyProviderRequestKindV1::ReleaseContribution(request) => Ok(request),
            ValidatedCustodyProviderRequestKindV1::ProvisionNodeShare(_) => {
                Err(ContractError::InvalidField("release_contribution"))
            }
        }
    }
}

impl ValidatedCustodyProvisionNodeShareRequestV1 {
    pub const fn selected_node_public_key(&self) -> NodePublicKey {
        self.selected_node_public_key
    }

    pub fn record_identity(&self) -> CustodyNodeProvisioningRecordIdentityV1 {
        self.authenticated_runtime_custody_provisioning
            .record_identity()
    }

    pub fn provisioning_id(&self) -> RuntimeCustodyProvisioningIdV1 {
        self.authenticated_runtime_custody_provisioning
            .provisioning_id()
    }

    pub fn provisioning_operation_hash(&self) -> Digest32 {
        self.authenticated_runtime_custody_provisioning
            .operation_hash()
    }

    pub fn custody_node_provisioning_record(&self) -> &CustodyNodeProvisioningRecordV1 {
        &self.custody_node_provisioning_record
    }

    pub fn authenticated_runtime_custody_provisioning(
        &self,
    ) -> &AuthenticatedRuntimeCustodyProvisioningV1 {
        &self.authenticated_runtime_custody_provisioning
    }
}

impl ValidatedCustodyReleaseContributionRequestV1 {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyProviderResponseStatusV1 {
    Provisioned,
    Contribution,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyProviderResponseV1(CustodyProviderResponseKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum CustodyProviderResponseKindV1 {
    Provisioned {
        schema: String,
        provisioning_id: [u8; 32],
        provisioning_operation_hash: [u8; 32],
        custody_node_provisioning_record_sha256: [u8; 32],
        custody_node_provisioning_record_bytes: u32,
        selected_node_public_key: [u8; 32],
    },
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
    pub fn status(&self) -> CustodyProviderResponseStatusV1 {
        match &self.0 {
            CustodyProviderResponseKindV1::Provisioned { .. } => {
                CustodyProviderResponseStatusV1::Provisioned
            }
            CustodyProviderResponseKindV1::Contribution { .. } => {
                CustodyProviderResponseStatusV1::Contribution
            }
            CustodyProviderResponseKindV1::Failure { .. } => {
                CustodyProviderResponseStatusV1::Failure
            }
        }
    }

    pub fn new_provisioned(
        request: &ValidatedCustodyProvisionNodeShareRequestV1,
    ) -> Result<Self, ContractError> {
        let value = Self(CustodyProviderResponseKindV1::Provisioned {
            schema: CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            provisioning_id: *request.provisioning_id().digest().as_bytes(),
            provisioning_operation_hash: *request.provisioning_operation_hash().as_bytes(),
            custody_node_provisioning_record_sha256: *request
                .record_identity()
                .record_sha256()
                .as_bytes(),
            custody_node_provisioning_record_bytes: request.record_identity().record_bytes(),
            selected_node_public_key: *request.selected_node_public_key().as_bytes(),
        });
        value.validate()?;
        Ok(value)
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
        request: &ValidatedCustodyReleaseContributionRequestV1,
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

    pub fn provisioned_id(&self) -> Result<RuntimeCustodyProvisioningIdV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Provisioned {
                provisioning_id, ..
            } => RuntimeCustodyProvisioningIdV1::new(Digest32::new(*provisioning_id)),
            CustodyProviderResponseKindV1::Contribution { .. }
            | CustodyProviderResponseKindV1::Failure { .. } => {
                Err(ContractError::InvalidField("provisioning_id"))
            }
        }
    }

    pub fn provisioned_operation_hash(&self) -> Result<Digest32, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Provisioned {
                provisioning_operation_hash,
                ..
            } => Ok(Digest32::new(*provisioning_operation_hash)),
            CustodyProviderResponseKindV1::Contribution { .. }
            | CustodyProviderResponseKindV1::Failure { .. } => {
                Err(ContractError::InvalidField("provisioning_operation_hash"))
            }
        }
    }

    pub fn provisioned_record_identity(
        &self,
    ) -> Result<CustodyNodeProvisioningRecordIdentityV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Provisioned {
                custody_node_provisioning_record_sha256,
                custody_node_provisioning_record_bytes,
                ..
            } => CustodyNodeProvisioningRecordIdentityV1::new(
                Digest32::new(*custody_node_provisioning_record_sha256),
                *custody_node_provisioning_record_bytes,
            ),
            CustodyProviderResponseKindV1::Contribution { .. }
            | CustodyProviderResponseKindV1::Failure { .. } => Err(ContractError::InvalidField(
                "custody_node_provisioning_record_identity",
            )),
        }
    }

    pub fn provisioned_selected_node_public_key(&self) -> Result<NodePublicKey, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Provisioned {
                selected_node_public_key,
                ..
            } => NodePublicKey::new(*selected_node_public_key),
            CustodyProviderResponseKindV1::Contribution { .. }
            | CustodyProviderResponseKindV1::Failure { .. } => {
                Err(ContractError::InvalidField("selected_node_public_key"))
            }
        }
    }

    pub fn signed_node_contribution(&self) -> Result<SignedNodeContributionV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Contribution {
                signed_node_contribution,
                ..
            } => signed_node_contribution.decode(),
            CustodyProviderResponseKindV1::Provisioned { .. }
            | CustodyProviderResponseKindV1::Failure { .. } => {
                Err(ContractError::InvalidField("signed_node_contribution"))
            }
        }
    }

    pub fn failure_code(&self) -> Result<ProviderFailureCodeV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Failure { code, .. } => Ok(*code),
            CustodyProviderResponseKindV1::Provisioned { .. }
            | CustodyProviderResponseKindV1::Contribution { .. } => {
                Err(ContractError::InvalidField("provider_failure_code"))
            }
        }
    }

    pub fn failure_audit_request_id(&self) -> Result<RuntimeReleaseAuditIdV1, ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Failure {
                audit_request_id, ..
            } => RuntimeReleaseAuditIdV1::new(Digest32::new(*audit_request_id)),
            CustodyProviderResponseKindV1::Provisioned { .. }
            | CustodyProviderResponseKindV1::Contribution { .. } => {
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
            CustodyProviderResponseKindV1::Provisioned { .. }
            | CustodyProviderResponseKindV1::Contribution { .. } => {
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
            CustodyProviderResponseKindV1::Provisioned { .. }
            | CustodyProviderResponseKindV1::Contribution { .. } => {
                Err(ContractError::InvalidField("selected_node_public_key"))
            }
        }
    }

    pub fn validate_against_request_at(
        &self,
        request: &CustodyProviderRequestV1,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        expected_local_node_public_key: NodePublicKey,
        now_unix_ms: u64,
    ) -> Result<(), ContractError> {
        match &self.0 {
            CustodyProviderResponseKindV1::Provisioned { .. } => {
                let validated = request.clone().into_validated_at(
                    expected_runtime_issuer,
                    expected_local_node_public_key,
                    now_unix_ms,
                )?;
                let provision = validated.provision_node_share()?;
                if self.provisioned_id()? != provision.provisioning_id() {
                    return Err(ContractError::InvalidField("provisioning_id"));
                }
                if self.provisioned_operation_hash()? != provision.provisioning_operation_hash() {
                    return Err(ContractError::InvalidField("provisioning_operation_hash"));
                }
                if self.provisioned_record_identity()? != provision.record_identity() {
                    return Err(ContractError::InvalidField(
                        "custody_node_provisioning_record_identity",
                    ));
                }
                if self.provisioned_selected_node_public_key()?
                    != provision.selected_node_public_key()
                {
                    return Err(ContractError::InvalidField("selected_node_public_key"));
                }
                Ok(())
            }
            CustodyProviderResponseKindV1::Contribution {
                signed_node_contribution,
                ..
            } => {
                let validated = request.clone().into_validated_at(
                    expected_runtime_issuer,
                    expected_local_node_public_key,
                    now_unix_ms,
                )?;
                let release = validated.release_contribution()?;
                let node_set = validated
                    .release_contribution()?
                    .authenticated_runtime_release_operation()
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let contribution: SignedNodeContributionV1 = signed_node_contribution.decode()?;
                let verified = release
                    .authenticated_runtime_release_operation()
                    .verify_node_contribution(&contribution, &node_set, now_unix_ms)
                    .map_err(|_| ContractError::InvalidField("signed_node_contribution"))?;
                if verified.node_public_key() != release.selected_node_public_key() {
                    return Err(ContractError::InvalidField("signed_node_contribution"));
                }
                Ok(())
            }
            CustodyProviderResponseKindV1::Failure { .. } => {
                let validated = request.clone().into_validated_at(
                    expected_runtime_issuer,
                    expected_local_node_public_key,
                    now_unix_ms,
                )?;
                let release = validated.release_contribution()?;
                let authenticated = release.authenticated_runtime_release_operation();
                if self.failure_audit_request_id()? != authenticated.statement().audit_request_id()
                {
                    return Err(ContractError::InvalidField("audit_request_id"));
                }
                if self.failure_release_request_hash()? != authenticated.release_request_hash() {
                    return Err(ContractError::InvalidField("release_request_hash"));
                }
                if self.failure_selected_node_public_key()? != release.selected_node_public_key() {
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
            CustodyProviderResponseKindV1::Provisioned { schema, .. } => {
                validate_schema(
                    schema,
                    CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
                    "custody_provider_response.schema",
                )?;
                let _ = self.provisioned_id()?;
                let _ = self.provisioned_operation_hash()?;
                let _ = self.provisioned_record_identity()?;
                let _ = self.provisioned_selected_node_public_key()?;
                Ok(())
            }
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
    use ed25519_dalek::{Signer as _, SigningKey};
    use elastos_protected_content_contracts::{
        CanonicalContract, CustodyNodeProvisioningRecordV1, RightsDecisionV1,
        RuntimeCustodyProvisioningIdV1, RuntimeCustodyProvisioningStatementV1,
        RuntimeOperationIssuerKeyV1, SignedRuntimeCustodyProvisioningV1,
    };

    use crate::{
        test_support::{
            custody_envelope_for_seed, digest, make_signed_node_contribution,
            make_signed_node_rights_decision, make_signed_runtime_release_operation,
            make_signed_runtime_release_operation_for_envelope_and_seed, node_public_key,
            runtime_operation_issuer_for_seed,
        },
        CustodyProviderRequestV1, CustodyProviderResponseStatusV1, CustodyProviderResponseV1,
        ProviderFailureCodeV1, ValidatedCustodyProviderRequestV1,
        CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
    };

    fn provisioning_record(seed: u8, node_seed: u8) -> CustodyNodeProvisioningRecordV1 {
        let envelope = custody_envelope_for_seed(seed);
        CustodyNodeProvisioningRecordV1::new(
            envelope.key_envelope_identity().unwrap(),
            envelope.manifest().clone(),
            node_public_key(node_seed),
            envelope
                .stored_share_for_node(node_public_key(node_seed))
                .unwrap()
                .clone(),
        )
        .unwrap()
    }

    fn signed_provisioning(
        record: &CustodyNodeProvisioningRecordV1,
        seed: u8,
    ) -> SignedRuntimeCustodyProvisioningV1 {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let statement = RuntimeCustodyProvisioningStatementV1::new(
            RuntimeOperationIssuerKeyV1::new(key.verifying_key().to_bytes()).unwrap(),
            record.record_identity().unwrap(),
            RuntimeCustodyProvisioningIdV1::new(digest(seed ^ 0x44)).unwrap(),
            crate::test_support::NOW + 1,
            crate::test_support::NOW + 40,
        )
        .unwrap();
        SignedRuntimeCustodyProvisioningV1::new(
            statement.clone(),
            key.sign(&statement.canonical_bytes().unwrap())
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    fn decode_provision_request(
        bytes: &[u8],
    ) -> Result<ValidatedCustodyProviderRequestV1, serde_json::Error> {
        ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            bytes,
            runtime_operation_issuer_for_seed(0x71),
            node_public_key(1),
            crate::test_support::NOW + 10,
        )
    }

    fn decode_release_request(
        bytes: &[u8],
    ) -> Result<ValidatedCustodyProviderRequestV1, serde_json::Error> {
        ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            bytes,
            runtime_operation_issuer_for_seed(0x42),
            node_public_key(1),
            crate::test_support::NOW + 10,
        )
    }

    #[test]
    fn custody_provision_request_and_receipt_round_trip_with_exact_binding() {
        let record = provisioning_record(0x11, 1);
        let signed = signed_provisioning(&record, 0x71);
        let request = CustodyProviderRequestV1::new_provision_node_share(&record, &signed).unwrap();
        assert_eq!(
            request.op(),
            crate::CustodyProviderRequestOpV1::ProvisionNodeShare
        );

        let validated = decode_provision_request(&request.to_json_vec().unwrap()).unwrap();
        let provision = validated.provision_node_share().unwrap();
        assert_eq!(provision.selected_node_public_key(), node_public_key(1));
        assert_eq!(
            provision.record_identity(),
            record.record_identity().unwrap()
        );
        assert_eq!(provision.custody_node_provisioning_record(), &record);
        assert_eq!(
            provision
                .authenticated_runtime_custody_provisioning()
                .statement(),
            signed.statement()
        );

        let response = CustodyProviderResponseV1::new_provisioned(provision).unwrap();
        assert_eq!(
            response.status(),
            CustodyProviderResponseStatusV1::Provisioned
        );
        assert_eq!(
            response.provisioned_id().unwrap(),
            signed.statement().provisioning_id()
        );
        assert_eq!(
            response.provisioned_operation_hash().unwrap(),
            signed.statement().canonical_hash().unwrap()
        );
        assert_eq!(
            response.provisioned_record_identity().unwrap(),
            record.record_identity().unwrap()
        );
        response
            .validate_against_request_at(
                &request,
                runtime_operation_issuer_for_seed(0x71),
                node_public_key(1),
                crate::test_support::NOW + 10,
            )
            .unwrap();

        let mut injected =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        injected["now_unix_ms"] = serde_json::json!(crate::test_support::NOW + 10);
        assert!(decode_provision_request(&serde_json::to_vec(&injected).unwrap()).is_err());
    }

    #[test]
    fn custody_request_requires_expected_runtime_issuer_and_local_node() {
        let record = provisioning_record(0x11, 1);
        let signed = signed_provisioning(&record, 0x71);
        let provision_request =
            CustodyProviderRequestV1::new_provision_node_share(&record, &signed).unwrap();
        assert!(ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &provision_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x72),
            node_public_key(1),
            crate::test_support::NOW + 10,
        )
        .is_err());
        assert!(ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &provision_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x71),
            node_public_key(2),
            crate::test_support::NOW + 10,
        )
        .is_err());

        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let release_request =
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
        assert!(ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &release_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x43),
            node_public_key(1),
            crate::test_support::NOW + 10,
        )
        .is_err());
        assert!(ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &release_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            node_public_key(2),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn custody_release_request_round_trips_without_envelope_or_share_bytes() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let request =
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
        let decoded =
            CustodyProviderRequestV1::decode_wire(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, request);
        let validated = decode_release_request(&request.to_json_vec().unwrap()).unwrap();
        let release = validated.release_contribution().unwrap();
        assert_eq!(validated.op(), request.op());
        assert_eq!(release.selected_node_public_key(), node_public_key(1));

        let mut json =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        assert!(json.get("custody_envelope").is_none());
        assert!(json.get("shares").is_none());
        json["custody_envelope"] = serde_json::json!([]);
        assert!(decode_release_request(&serde_json::to_vec(&json).unwrap()).is_err());
    }

    #[test]
    fn custody_request_rejects_schema_bounds_missing_duplicate_trailing_and_aggregate_fields() {
        let record = provisioning_record(0x11, 1);
        let signed = signed_provisioning(&record, 0x71);
        let request = CustodyProviderRequestV1::new_provision_node_share(&record, &signed).unwrap();
        let request_json = request.to_json_vec().unwrap();

        let mut wrong_schema: serde_json::Value = serde_json::from_slice(&request_json).unwrap();
        wrong_schema["schema"] = serde_json::json!("wrong.schema/v1");
        assert!(decode_provision_request(&serde_json::to_vec(&wrong_schema).unwrap()).is_err());

        let mut missing_record: serde_json::Value = serde_json::from_slice(&request_json).unwrap();
        missing_record
            .as_object_mut()
            .unwrap()
            .remove("custody_node_provisioning_record");
        assert!(decode_provision_request(&serde_json::to_vec(&missing_record).unwrap()).is_err());

        let mut extra: serde_json::Value = serde_json::from_slice(&request_json).unwrap();
        extra["custody_envelope"] = serde_json::json!([]);
        extra["shares"] = serde_json::json!([]);
        assert!(decode_provision_request(&serde_json::to_vec(&extra).unwrap()).is_err());

        let with_trailing = [request_json.as_slice(), b" nope"].concat();
        assert!(decode_provision_request(&with_trailing).is_err());

        let text = String::from_utf8(request_json).unwrap();
        let duplicate = text.replacen(
            "\"schema\":",
            &format!("\"schema\":\"{CUSTODY_PROVIDER_REQUEST_SCHEMA_V1}\",\"schema\":"),
            1,
        );
        assert!(decode_provision_request(duplicate.as_bytes()).is_err());

        let oversized = vec![b' '; crate::MAX_PROVIDER_FRAME_BYTES_V1.saturating_add(1)];
        assert!(decode_provision_request(&oversized).is_err());
    }

    #[test]
    fn custody_provision_request_rejects_record_identity_and_selected_node_substitution() {
        let record = provisioning_record(0x11, 1);
        let signed = signed_provisioning(&record, 0x71);
        let other_record = provisioning_record(0x22, 2);
        let other_signed = signed_provisioning(&other_record, 0x72);

        let mut mismatched_record = serde_json::from_slice::<serde_json::Value>(
            &CustodyProviderRequestV1::new_provision_node_share(&record, &signed)
                .unwrap()
                .to_json_vec()
                .unwrap(),
        )
        .unwrap();
        mismatched_record["custody_node_provisioning_record"] =
            serde_json::to_value(other_record.canonical_bytes().unwrap()).unwrap();
        assert!(
            decode_provision_request(&serde_json::to_vec(&mismatched_record).unwrap()).is_err()
        );

        let other_request =
            CustodyProviderRequestV1::new_provision_node_share(&other_record, &other_signed)
                .unwrap();
        let response = {
            let validated = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
                &other_request.to_json_vec().unwrap(),
                runtime_operation_issuer_for_seed(0x72),
                node_public_key(2),
                crate::test_support::NOW + 10,
            )
            .unwrap();
            CustodyProviderResponseV1::new_provisioned(validated.provision_node_share().unwrap())
                .unwrap()
        };
        let original_request =
            CustodyProviderRequestV1::new_provision_node_share(&record, &signed).unwrap();
        assert!(response
            .validate_against_request_at(
                &original_request,
                runtime_operation_issuer_for_seed(0x71),
                node_public_key(1),
                crate::test_support::NOW + 10,
            )
            .is_err());
    }

    #[test]
    fn custody_release_rejects_wrong_decision_binding_and_node_outside_committee() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();

        let other_operation = make_signed_runtime_release_operation_for_envelope_and_seed(
            0x52,
            &custody_envelope_for_seed(0x52),
        );
        let wrong_decision_request = CustodyProviderRequestV1::new_release_contribution(
            &operation,
            &make_signed_node_rights_decision(&other_operation, 1, RightsDecisionV1::Allowed),
        )
        .unwrap();
        assert!(decode_release_request(&wrong_decision_request.to_json_vec().unwrap()).is_err());

        let outsider_decision =
            make_signed_node_rights_decision(&operation, 9, RightsDecisionV1::Allowed);
        assert!(
            CustodyProviderRequestV1::new_release_contribution(&operation, &outsider_decision)
                .is_err()
        );
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
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap()
        };
        decoded
            .validate_against_request_at(
                &request,
                runtime_operation_issuer_for_seed(0x42),
                node_public_key(1),
                crate::test_support::NOW + 10,
            )
            .unwrap();
        assert!(
            CustodyProviderResponseV1::from_json_slice(&request.to_json_vec().unwrap()).is_err()
        );
    }

    #[test]
    fn custody_response_failure_is_typed() {
        let operation = make_signed_runtime_release_operation();
        let decision = make_signed_node_rights_decision(&operation, 1, RightsDecisionV1::Allowed);
        let request =
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
        let validated = decode_release_request(&request.to_json_vec().unwrap()).unwrap();
        let release = validated.release_contribution().unwrap();
        let response =
            CustodyProviderResponseV1::new_failure(release, ProviderFailureCodeV1::NotConfigured)
                .unwrap();
        assert_eq!(response.status(), CustodyProviderResponseStatusV1::Failure);
        assert_eq!(
            response.failure_code().unwrap(),
            ProviderFailureCodeV1::NotConfigured
        );
        assert_eq!(
            response.failure_audit_request_id().unwrap(),
            release
                .authenticated_runtime_release_operation()
                .statement()
                .audit_request_id()
        );
        assert_eq!(
            response.failure_release_request_hash().unwrap(),
            release
                .authenticated_runtime_release_operation()
                .release_request_hash()
        );
        assert_eq!(
            response.failure_selected_node_public_key().unwrap(),
            release.selected_node_public_key()
        );
        response
            .validate_against_request_at(
                &request,
                runtime_operation_issuer_for_seed(0x42),
                node_public_key(1),
                crate::test_support::NOW + 10,
            )
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
        let request =
            CustodyProviderRequestV1::new_release_contribution(&operation, &decision).unwrap();
        let other_request =
            CustodyProviderRequestV1::new_release_contribution(&other_operation, &other_decision)
                .unwrap();
        let validated_other = ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            &other_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x44),
            node_public_key(1),
            crate::test_support::NOW + 10,
        )
        .unwrap();
        let response = CustodyProviderResponseV1::new_failure(
            validated_other.release_contribution().unwrap(),
            ProviderFailureCodeV1::BackendUnavailable,
        )
        .unwrap();
        assert!(response
            .validate_against_request_at(
                &request,
                runtime_operation_issuer_for_seed(0x42),
                node_public_key(1),
                crate::test_support::NOW + 10,
            )
            .is_err());
    }
}
