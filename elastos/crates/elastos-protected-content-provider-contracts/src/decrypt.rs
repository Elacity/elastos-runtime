use elastos_protected_content_contracts::{
    AuthenticatedRuntimeReleaseOperationV1, CanonicalContract, ContractError, CustodyEnvelopeV1,
    KeyReleaseOutcomeV1, ProtectedContentBindingV1, RecipientKeyIdentityV1,
    RecipientPublicKeyBytesV1, RightsActionV1, RuntimeOperationIssuerKeyV1,
    RuntimeReleaseAuditIdV1, SignedNodeContributionV1, SignedRuntimeReleaseOperationV1,
    SignedTerminalReceiptV1, TerminalReceiptIssuerKey,
    MAX_RECIPIENT_KEY_AUTHORIZATION_LIFETIME_SECS,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::media::{
    CencFmp4MediaIdentityV1, ValidatedCencFmp4MediaSessionLayoutV1,
    MAX_CENC_FMP4_MEDIA_IDENTITY_BYTES_V1,
};
use crate::wire::{
    contract_decode_error, decode_json, encode_json, validate_schema, validate_time_window,
    CanonicalBlob, CanonicalBlobList, OpaqueHandleV1, ProviderFailureCodeV1,
    MAX_CUSTODY_ENVELOPE_BYTES_V1, MAX_PROVIDER_BINDING_BYTES_V1,
    MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1, MAX_RECIPIENT_IDENTITY_BYTES_V1,
    MAX_SIGNED_NODE_CONTRIBUTIONS_COUNT_V1, MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1,
    MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1, MAX_SIGNED_TERMINAL_RECEIPT_BYTES_V1,
};

pub const DECRYPT_PROVIDER_REQUEST_SCHEMA_V1: &str =
    "elastos.protected-content.decrypt-provider.request/v1";
pub const DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1: &str =
    "elastos.protected-content.decrypt-provider.response/v1";
pub const MAX_VIEWER_MEDIA_PART_BYTES_V1: usize = 2 * 1024 * 1024;

type BindingBlobV1 = CanonicalBlob<MAX_PROVIDER_BINDING_BYTES_V1>;
type RecipientIdentityBlobV1 = CanonicalBlob<MAX_RECIPIENT_IDENTITY_BYTES_V1>;
type SignedRuntimeReleaseOperationBlobV1 =
    CanonicalBlob<MAX_SIGNED_RUNTIME_RELEASE_OPERATION_BYTES_V1>;
type CustodyEnvelopeBlobV1 = CanonicalBlob<MAX_CUSTODY_ENVELOPE_BYTES_V1>;
type MediaIdentityBlobV1 = CanonicalBlob<MAX_CENC_FMP4_MEDIA_IDENTITY_BYTES_V1>;
type MediaPartBlobV1 = CanonicalBlob<MAX_VIEWER_MEDIA_PART_BYTES_V1>;
type SignedNodeContributionBlobV1 = CanonicalBlob<MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1>;
type SignedNodeContributionBlobListV1 = CanonicalBlobList<
    MAX_SIGNED_NODE_CONTRIBUTIONS_COUNT_V1,
    MAX_SIGNED_NODE_CONTRIBUTION_BYTES_V1,
>;
type SignedTerminalReceiptBlobV1 = CanonicalBlob<MAX_SIGNED_TERMINAL_RECEIPT_BYTES_V1>;

fn decode_rights_action(value: u8) -> Result<RightsActionV1, ContractError> {
    match value {
        x if x == RightsActionV1::View as u8 => Ok(RightsActionV1::View),
        x if x == RightsActionV1::Stream as u8 => Ok(RightsActionV1::Stream),
        x if x == RightsActionV1::Download as u8 => Ok(RightsActionV1::Download),
        x if x == RightsActionV1::Execute as u8 => Ok(RightsActionV1::Execute),
        _ => Err(ContractError::InvalidField("rights_action")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecryptProviderRequestOpV1 {
    PrepareRecipient,
    OpenViewerSession,
    ReadViewerMediaPart,
    CancelPreparedRecipient,
    CloseViewerSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerMediaPartSelectorV1(ViewerMediaPartSelectorKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case", deny_unknown_fields)]
enum ViewerMediaPartSelectorKindV1 {
    Init {},
    Segment {
        segment_index: u32,
        encrypted_segment: MediaPartBlobV1,
    },
}

impl ViewerMediaPartSelectorV1 {
    pub const fn init() -> Self {
        Self(ViewerMediaPartSelectorKindV1::Init {})
    }

    pub fn segment(segment_index: u32, encrypted_segment: Vec<u8>) -> Result<Self, ContractError> {
        let value = Self(ViewerMediaPartSelectorKindV1::Segment {
            segment_index,
            encrypted_segment: MediaPartBlobV1::new(encrypted_segment)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub const fn is_init(&self) -> bool {
        matches!(self.0, ViewerMediaPartSelectorKindV1::Init { .. })
    }

    pub fn segment_index(&self) -> Option<u32> {
        match self.0 {
            ViewerMediaPartSelectorKindV1::Init { .. } => None,
            ViewerMediaPartSelectorKindV1::Segment { segment_index, .. } => Some(segment_index),
        }
    }

    pub fn encrypted_segment(&self) -> Option<&[u8]> {
        match &self.0 {
            ViewerMediaPartSelectorKindV1::Init { .. } => None,
            ViewerMediaPartSelectorKindV1::Segment {
                encrypted_segment, ..
            } => Some(encrypted_segment.as_slice()),
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        match &self.0 {
            ViewerMediaPartSelectorKindV1::Init { .. } => Ok(()),
            ViewerMediaPartSelectorKindV1::Segment {
                encrypted_segment, ..
            } => {
                let _ = encrypted_segment.as_slice();
                Ok(())
            }
        }
    }
}

impl Serialize for ViewerMediaPartSelectorV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ViewerMediaPartSelectorV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = ViewerMediaPartSelectorKindV1::deserialize(deserializer)?;
        let value = Self(inner);
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptProviderRequestV1(DecryptProviderRequestKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum DecryptProviderRequestKindV1 {
    PrepareRecipient {
        schema: String,
        protected_content_binding: BindingBlobV1,
        audit_request_id: [u8; 32],
        action: u8,
        runtime_operation_issuer: [u8; 32],
        issued_at: u64,
        expires_at: u64,
    },
    OpenViewerSession {
        schema: String,
        prepared_recipient_handle: OpaqueHandleV1,
        signed_runtime_release_operation: SignedRuntimeReleaseOperationBlobV1,
        expected_terminal_issuer: [u8; 32],
        custody_envelope: CustodyEnvelopeBlobV1,
        media_identity: MediaIdentityBlobV1,
        protected_init_segment: MediaPartBlobV1,
        signed_node_contributions: SignedNodeContributionBlobListV1,
        signed_terminal_receipt: SignedTerminalReceiptBlobV1,
    },
    ReadViewerMediaPart {
        schema: String,
        audit_request_id: [u8; 32],
        viewer_session_handle: OpaqueHandleV1,
        part_selector: ViewerMediaPartSelectorV1,
    },
    CancelPreparedRecipient {
        schema: String,
        audit_request_id: [u8; 32],
        prepared_recipient_handle: OpaqueHandleV1,
    },
    CloseViewerSession {
        schema: String,
        audit_request_id: [u8; 32],
        viewer_session_handle: OpaqueHandleV1,
    },
}

impl DecryptProviderRequestV1 {
    pub const fn op(&self) -> DecryptProviderRequestOpV1 {
        match self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient { .. } => {
                DecryptProviderRequestOpV1::PrepareRecipient
            }
            DecryptProviderRequestKindV1::OpenViewerSession { .. } => {
                DecryptProviderRequestOpV1::OpenViewerSession
            }
            DecryptProviderRequestKindV1::ReadViewerMediaPart { .. } => {
                DecryptProviderRequestOpV1::ReadViewerMediaPart
            }
            DecryptProviderRequestKindV1::CancelPreparedRecipient { .. } => {
                DecryptProviderRequestOpV1::CancelPreparedRecipient
            }
            DecryptProviderRequestKindV1::CloseViewerSession { .. } => {
                DecryptProviderRequestOpV1::CloseViewerSession
            }
        }
    }

    pub fn new_prepare_recipient(
        protected_content_binding: &ProtectedContentBindingV1,
        audit_request_id: RuntimeReleaseAuditIdV1,
        action: RightsActionV1,
        runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderRequestKindV1::PrepareRecipient {
            schema: DECRYPT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            protected_content_binding: CanonicalBlob::from_contract(protected_content_binding)?,
            audit_request_id: *audit_request_id.digest().as_bytes(),
            action: action as u8,
            runtime_operation_issuer: *runtime_operation_issuer.as_bytes(),
            issued_at,
            expires_at,
        });
        value.validate_structure()?;
        Ok(value)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the protocol requires these independently validated signed and media bindings"
    )]
    pub fn new_open_viewer_session(
        prepared_recipient_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        signed_runtime_release_operation: &SignedRuntimeReleaseOperationV1,
        expected_terminal_issuer: TerminalReceiptIssuerKey,
        custody_envelope: &CustodyEnvelopeV1,
        media_identity: &CencFmp4MediaIdentityV1,
        protected_init_segment: &[u8],
        signed_node_contributions: &[SignedNodeContributionV1],
        signed_terminal_receipt: &SignedTerminalReceiptV1,
    ) -> Result<Self, ContractError> {
        let contributions = signed_node_contributions
            .iter()
            .map(CanonicalBlob::from_contract)
            .collect::<Result<Vec<SignedNodeContributionBlobV1>, _>>()?;
        let value = Self(DecryptProviderRequestKindV1::OpenViewerSession {
            schema: DECRYPT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            prepared_recipient_handle: OpaqueHandleV1::new(prepared_recipient_handle)?,
            signed_runtime_release_operation: CanonicalBlob::from_contract(
                signed_runtime_release_operation,
            )?,
            expected_terminal_issuer: *expected_terminal_issuer.as_bytes(),
            custody_envelope: CanonicalBlob::from_contract(custody_envelope)?,
            media_identity: CanonicalBlob::from_contract(media_identity)?,
            protected_init_segment: MediaPartBlobV1::new(protected_init_segment.to_vec())?,
            signed_node_contributions: CanonicalBlobList::new(contributions)?,
            signed_terminal_receipt: CanonicalBlob::from_contract(signed_terminal_receipt)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_read_viewer_media_part(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        part_selector: ViewerMediaPartSelectorV1,
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderRequestKindV1::ReadViewerMediaPart {
            schema: DECRYPT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            viewer_session_handle: OpaqueHandleV1::new(viewer_session_handle)?,
            part_selector,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_cancel_prepared_recipient(
        audit_request_id: RuntimeReleaseAuditIdV1,
        prepared_recipient_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderRequestKindV1::CancelPreparedRecipient {
            schema: DECRYPT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            prepared_recipient_handle: OpaqueHandleV1::new(prepared_recipient_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_close_viewer_session(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderRequestKindV1::CloseViewerSession {
            schema: DECRYPT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            viewer_session_handle: OpaqueHandleV1::new(viewer_session_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    fn audit_request_id(&self) -> Result<RuntimeReleaseAuditIdV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient {
                audit_request_id, ..
            }
            | DecryptProviderRequestKindV1::CancelPreparedRecipient {
                audit_request_id, ..
            }
            | DecryptProviderRequestKindV1::CloseViewerSession {
                audit_request_id, ..
            }
            | DecryptProviderRequestKindV1::ReadViewerMediaPart {
                audit_request_id, ..
            } => RuntimeReleaseAuditIdV1::new(elastos_protected_content_contracts::Digest32::new(
                *audit_request_id,
            )),
            DecryptProviderRequestKindV1::OpenViewerSession {
                signed_runtime_release_operation,
                ..
            } => {
                let operation: SignedRuntimeReleaseOperationV1 =
                    signed_runtime_release_operation.decode()?;
                Ok(operation.statement().audit_request_id())
            }
        }
    }

    fn protected_content_binding(&self) -> Result<ProtectedContentBindingV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient {
                protected_content_binding,
                ..
            } => protected_content_binding.decode(),
            _ => Err(ContractError::InvalidField("protected_content_binding")),
        }
    }

    fn action(&self) -> Result<RightsActionV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient { action, .. } => {
                decode_rights_action(*action)
            }
            _ => Err(ContractError::InvalidField("rights_action")),
        }
    }

    fn runtime_operation_issuer(&self) -> Result<RuntimeOperationIssuerKeyV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient {
                runtime_operation_issuer,
                ..
            } => RuntimeOperationIssuerKeyV1::new(*runtime_operation_issuer),
            _ => Err(ContractError::InvalidField("runtime_operation_issuer")),
        }
    }

    fn issued_at(&self) -> Result<u64, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient { issued_at, .. } => Ok(*issued_at),
            _ => Err(ContractError::InvalidField("issued_at")),
        }
    }

    fn expires_at(&self) -> Result<u64, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient { expires_at, .. } => Ok(*expires_at),
            _ => Err(ContractError::InvalidField("expires_at")),
        }
    }

    fn prepared_recipient_handle(
        &self,
    ) -> Result<&[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                prepared_recipient_handle,
                ..
            }
            | DecryptProviderRequestKindV1::CancelPreparedRecipient {
                prepared_recipient_handle,
                ..
            } => Ok(prepared_recipient_handle.as_bytes()),
            _ => Err(ContractError::InvalidField("prepared_recipient_handle")),
        }
    }

    fn signed_runtime_release_operation(
        &self,
    ) -> Result<SignedRuntimeReleaseOperationV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                signed_runtime_release_operation,
                ..
            } => signed_runtime_release_operation.decode(),
            _ => Err(ContractError::InvalidField(
                "signed_runtime_release_operation",
            )),
        }
    }

    fn expected_terminal_issuer(&self) -> Result<TerminalReceiptIssuerKey, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                expected_terminal_issuer,
                ..
            } => TerminalReceiptIssuerKey::new(*expected_terminal_issuer),
            _ => Err(ContractError::InvalidField("expected_terminal_issuer")),
        }
    }

    fn custody_envelope(&self) -> Result<CustodyEnvelopeV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                custody_envelope, ..
            } => custody_envelope.decode(),
            _ => Err(ContractError::InvalidField("custody_envelope")),
        }
    }

    fn media_identity(&self) -> Result<CencFmp4MediaIdentityV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession { media_identity, .. } => {
                media_identity.decode()
            }
            _ => Err(ContractError::InvalidField("media_identity")),
        }
    }

    fn protected_init_segment(&self) -> Result<&[u8], ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                protected_init_segment,
                ..
            } => Ok(protected_init_segment.as_slice()),
            _ => Err(ContractError::InvalidField("protected_init_segment")),
        }
    }

    fn signed_node_contributions(&self) -> Result<Vec<SignedNodeContributionV1>, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                signed_node_contributions,
                ..
            } => signed_node_contributions
                .iter()
                .map(SignedNodeContributionV1::from_canonical_bytes)
                .collect(),
            _ => Err(ContractError::InvalidField("signed_node_contributions")),
        }
    }

    fn signed_terminal_receipt(&self) -> Result<SignedTerminalReceiptV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::OpenViewerSession {
                signed_terminal_receipt,
                ..
            } => signed_terminal_receipt.decode(),
            _ => Err(ContractError::InvalidField("signed_terminal_receipt")),
        }
    }

    fn viewer_session_handle(
        &self,
    ) -> Result<&[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::ReadViewerMediaPart {
                viewer_session_handle,
                ..
            }
            | DecryptProviderRequestKindV1::CloseViewerSession {
                viewer_session_handle,
                ..
            } => Ok(viewer_session_handle.as_bytes()),
            _ => Err(ContractError::InvalidField("viewer_session_handle")),
        }
    }

    fn viewer_media_part_selector(&self) -> Result<&ViewerMediaPartSelectorV1, ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::ReadViewerMediaPart { part_selector, .. } => {
                Ok(part_selector)
            }
            _ => Err(ContractError::InvalidField("part_selector")),
        }
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        match &self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
                    "decrypt_provider_request.schema",
                )?;
                self.protected_content_binding()?;
                self.audit_request_id()?;
                self.action()?;
                self.runtime_operation_issuer()?;
                validate_time_window(
                    self.issued_at()?,
                    self.expires_at()?,
                    MAX_RECIPIENT_KEY_AUTHORIZATION_LIFETIME_SECS,
                    "decrypt_prepare_window",
                )?;
                Ok(())
            }
            DecryptProviderRequestKindV1::OpenViewerSession { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
                    "decrypt_provider_request.schema",
                )?;
                self.prepared_recipient_handle()?;
                let _ = self.signed_runtime_release_operation()?;
                let _ = self.expected_terminal_issuer()?;
                let _ = self.custody_envelope()?;
                let _ = self.media_identity()?;
                let _ = self.protected_init_segment()?;
                let _ = self.signed_node_contributions()?;
                let _ = self.signed_terminal_receipt()?;
                Ok(())
            }
            DecryptProviderRequestKindV1::ReadViewerMediaPart { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
                    "decrypt_provider_request.schema",
                )?;
                self.audit_request_id()?;
                self.viewer_session_handle()?;
                let _ = self.viewer_media_part_selector()?;
                Ok(())
            }
            DecryptProviderRequestKindV1::CancelPreparedRecipient { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
                    "decrypt_provider_request.schema",
                )?;
                self.audit_request_id()?;
                self.prepared_recipient_handle()?;
                Ok(())
            }
            DecryptProviderRequestKindV1::CloseViewerSession { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
                    "decrypt_provider_request.schema",
                )?;
                self.audit_request_id()?;
                self.viewer_session_handle()?;
                Ok(())
            }
        }
    }

    fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let inner = decode_json::<DecryptProviderRequestKindV1>(bytes)?;
        let value = Self(inner);
        value.validate_structure().map_err(contract_decode_error)?;
        Ok(value)
    }

    fn into_validated_at(
        self,
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now_unix_seconds: u64,
    ) -> Result<ValidatedDecryptProviderRequestV1, ContractError> {
        match self.0 {
            DecryptProviderRequestKindV1::PrepareRecipient {
                protected_content_binding,
                audit_request_id,
                action,
                runtime_operation_issuer,
                issued_at,
                expires_at,
                ..
            } => {
                if now_unix_seconds < issued_at || now_unix_seconds >= expires_at {
                    return Err(ContractError::InvalidField("decrypt_prepare_window"));
                }
                let runtime_operation_issuer =
                    RuntimeOperationIssuerKeyV1::new(runtime_operation_issuer)?;
                if runtime_operation_issuer != expected_runtime_issuer {
                    return Err(ContractError::InvalidField("runtime_operation_issuer"));
                }
                Ok(ValidatedDecryptProviderRequestV1(
                    ValidatedDecryptProviderRequestKindV1::PrepareRecipient {
                        protected_content_binding: protected_content_binding.decode()?,
                        audit_request_id: RuntimeReleaseAuditIdV1::new(
                            elastos_protected_content_contracts::Digest32::new(audit_request_id),
                        )?,
                        action: decode_rights_action(action)?,
                        runtime_operation_issuer,
                        issued_at,
                        expires_at,
                    },
                ))
            }
            DecryptProviderRequestKindV1::OpenViewerSession {
                prepared_recipient_handle,
                signed_runtime_release_operation,
                expected_terminal_issuer,
                custody_envelope,
                media_identity,
                protected_init_segment,
                signed_node_contributions,
                signed_terminal_receipt,
                ..
            } => {
                let authenticated_runtime_release_operation = signed_runtime_release_operation
                    .decode::<SignedRuntimeReleaseOperationV1>()?
                    .verify(expected_runtime_issuer, now_unix_seconds)
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let expected_terminal_issuer =
                    TerminalReceiptIssuerKey::new(expected_terminal_issuer)?;
                let custody_envelope: CustodyEnvelopeV1 = custody_envelope.decode()?;
                let media_identity: CencFmp4MediaIdentityV1 = media_identity.decode()?;
                if media_identity.encrypted_content()
                    != custody_envelope.manifest().encrypted_content()
                {
                    return Err(ContractError::InvalidField("media_identity"));
                }
                if media_identity.encrypted_content()
                    != authenticated_runtime_release_operation
                        .statement()
                        .release_request()
                        .binding()
                        .encrypted_content()
                {
                    return Err(ContractError::InvalidField("media_identity"));
                }
                let media_session_layout = ValidatedCencFmp4MediaSessionLayoutV1::new(
                    &media_identity,
                    protected_init_segment.as_slice(),
                )
                .map_err(|_| ContractError::InvalidField("protected_init_segment"))?;
                let signed_node_contributions: Vec<SignedNodeContributionV1> =
                    signed_node_contributions
                        .iter()
                        .map(SignedNodeContributionV1::from_canonical_bytes)
                        .collect::<Result<_, _>>()?;
                let signed_terminal_receipt: SignedTerminalReceiptV1 =
                    signed_terminal_receipt.decode()?;
                let node_set = authenticated_runtime_release_operation
                    .statement()
                    .custody_epoch()
                    .statement()
                    .node_set()
                    .map_err(|_| ContractError::InvalidField("signed_runtime_release_operation"))?;
                let mut verified_contributions =
                    Vec::with_capacity(signed_node_contributions.len());
                for contribution in &signed_node_contributions {
                    let verified = authenticated_runtime_release_operation
                        .verify_node_contribution(contribution, &node_set, now_unix_seconds)
                        .map_err(|_| ContractError::InvalidField("signed_node_contributions"))?;
                    authenticated_runtime_release_operation
                        .validate_node_release_claim_context(
                            &custody_envelope,
                            verified.node_public_key(),
                            now_unix_seconds,
                        )
                        .map_err(|_| ContractError::InvalidField("custody_envelope"))?;
                    verified_contributions.push(verified);
                }
                if signed_terminal_receipt.statement().outcome() != KeyReleaseOutcomeV1::Released {
                    return Err(ContractError::InvalidField("signed_terminal_receipt"));
                }
                authenticated_runtime_release_operation
                    .verify_terminal_receipt(
                        &signed_terminal_receipt,
                        &verified_contributions,
                        expected_terminal_issuer,
                        now_unix_seconds,
                    )
                    .map_err(|_| ContractError::InvalidField("signed_terminal_receipt"))?;
                Ok(ValidatedDecryptProviderRequestV1(
                    ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                        prepared_recipient_handle,
                        authenticated_runtime_release_operation: Box::new(
                            authenticated_runtime_release_operation,
                        ),
                        expected_terminal_issuer,
                        custody_envelope,
                        media_session_layout: Box::new(media_session_layout),
                        protected_init_segment,
                        signed_node_contributions,
                        signed_terminal_receipt: Box::new(signed_terminal_receipt),
                    },
                ))
            }
            DecryptProviderRequestKindV1::ReadViewerMediaPart {
                audit_request_id,
                viewer_session_handle,
                part_selector,
                ..
            } => Ok(ValidatedDecryptProviderRequestV1(
                ValidatedDecryptProviderRequestKindV1::ReadViewerMediaPart {
                    audit_request_id: RuntimeReleaseAuditIdV1::new(
                        elastos_protected_content_contracts::Digest32::new(audit_request_id),
                    )?,
                    viewer_session_handle,
                    part_selector,
                },
            )),
            DecryptProviderRequestKindV1::CancelPreparedRecipient {
                audit_request_id,
                prepared_recipient_handle,
                ..
            } => Ok(ValidatedDecryptProviderRequestV1(
                ValidatedDecryptProviderRequestKindV1::CancelPreparedRecipient {
                    audit_request_id: RuntimeReleaseAuditIdV1::new(
                        elastos_protected_content_contracts::Digest32::new(audit_request_id),
                    )?,
                    prepared_recipient_handle,
                },
            )),
            DecryptProviderRequestKindV1::CloseViewerSession {
                audit_request_id,
                viewer_session_handle,
                ..
            } => Ok(ValidatedDecryptProviderRequestV1(
                ValidatedDecryptProviderRequestKindV1::CloseViewerSession {
                    audit_request_id: RuntimeReleaseAuditIdV1::new(
                        elastos_protected_content_contracts::Digest32::new(audit_request_id),
                    )?,
                    viewer_session_handle,
                },
            )),
        }
    }
}

impl Serialize for DecryptProviderRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ValidatedDecryptProviderRequestKindV1 {
    PrepareRecipient {
        protected_content_binding: ProtectedContentBindingV1,
        audit_request_id: RuntimeReleaseAuditIdV1,
        action: RightsActionV1,
        runtime_operation_issuer: RuntimeOperationIssuerKeyV1,
        issued_at: u64,
        expires_at: u64,
    },
    OpenViewerSession {
        prepared_recipient_handle: OpaqueHandleV1,
        authenticated_runtime_release_operation: Box<AuthenticatedRuntimeReleaseOperationV1>,
        expected_terminal_issuer: TerminalReceiptIssuerKey,
        custody_envelope: CustodyEnvelopeV1,
        media_session_layout: Box<ValidatedCencFmp4MediaSessionLayoutV1>,
        protected_init_segment: MediaPartBlobV1,
        signed_node_contributions: Vec<SignedNodeContributionV1>,
        signed_terminal_receipt: Box<SignedTerminalReceiptV1>,
    },
    ReadViewerMediaPart {
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: OpaqueHandleV1,
        part_selector: ViewerMediaPartSelectorV1,
    },
    CancelPreparedRecipient {
        audit_request_id: RuntimeReleaseAuditIdV1,
        prepared_recipient_handle: OpaqueHandleV1,
    },
    CloseViewerSession {
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: OpaqueHandleV1,
    },
}

/// Provider-authoritative decrypt request. Construct only through
/// [`ValidatedDecryptProviderRequestV1::decode_and_validate_at`].
///
/// ```compile_fail
/// use elastos_protected_content_provider_contracts::ValidatedDecryptProviderRequestV1;
///
/// let _ = serde_json::from_slice::<ValidatedDecryptProviderRequestV1>(br#"{}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDecryptProviderRequestV1(ValidatedDecryptProviderRequestKindV1);

impl ValidatedDecryptProviderRequestV1 {
    pub fn decode_and_validate_at(
        bytes: &[u8],
        expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
        now_unix_seconds: u64,
    ) -> Result<Self, serde_json::Error> {
        DecryptProviderRequestV1::decode_wire(bytes)?
            .into_validated_at(expected_runtime_issuer, now_unix_seconds)
            .map_err(contract_decode_error)
    }

    pub const fn op(&self) -> DecryptProviderRequestOpV1 {
        match self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient { .. } => {
                DecryptProviderRequestOpV1::PrepareRecipient
            }
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession { .. } => {
                DecryptProviderRequestOpV1::OpenViewerSession
            }
            ValidatedDecryptProviderRequestKindV1::ReadViewerMediaPart { .. } => {
                DecryptProviderRequestOpV1::ReadViewerMediaPart
            }
            ValidatedDecryptProviderRequestKindV1::CancelPreparedRecipient { .. } => {
                DecryptProviderRequestOpV1::CancelPreparedRecipient
            }
            ValidatedDecryptProviderRequestKindV1::CloseViewerSession { .. } => {
                DecryptProviderRequestOpV1::CloseViewerSession
            }
        }
    }

    pub fn audit_request_id(&self) -> RuntimeReleaseAuditIdV1 {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient {
                audit_request_id, ..
            }
            | ValidatedDecryptProviderRequestKindV1::CancelPreparedRecipient {
                audit_request_id,
                ..
            }
            | ValidatedDecryptProviderRequestKindV1::CloseViewerSession {
                audit_request_id, ..
            }
            | ValidatedDecryptProviderRequestKindV1::ReadViewerMediaPart {
                audit_request_id, ..
            } => *audit_request_id,
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                authenticated_runtime_release_operation,
                ..
            } => authenticated_runtime_release_operation
                .statement()
                .audit_request_id(),
        }
    }

    pub fn protected_content_binding(&self) -> Result<&ProtectedContentBindingV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient {
                protected_content_binding,
                ..
            } => Ok(protected_content_binding),
            _ => Err(ContractError::InvalidField("protected_content_binding")),
        }
    }

    pub fn action(&self) -> Result<RightsActionV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient { action, .. } => Ok(*action),
            _ => Err(ContractError::InvalidField("rights_action")),
        }
    }

    pub fn runtime_operation_issuer(&self) -> Result<RuntimeOperationIssuerKeyV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient {
                runtime_operation_issuer,
                ..
            } => Ok(*runtime_operation_issuer),
            _ => Err(ContractError::InvalidField("runtime_operation_issuer")),
        }
    }

    pub fn issued_at(&self) -> Result<u64, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient { issued_at, .. } => {
                Ok(*issued_at)
            }
            _ => Err(ContractError::InvalidField("issued_at")),
        }
    }

    pub fn expires_at(&self) -> Result<u64, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::PrepareRecipient { expires_at, .. } => {
                Ok(*expires_at)
            }
            _ => Err(ContractError::InvalidField("expires_at")),
        }
    }

    pub fn prepared_recipient_handle(
        &self,
    ) -> Result<&[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                prepared_recipient_handle,
                ..
            }
            | ValidatedDecryptProviderRequestKindV1::CancelPreparedRecipient {
                prepared_recipient_handle,
                ..
            } => Ok(prepared_recipient_handle.as_bytes()),
            _ => Err(ContractError::InvalidField("prepared_recipient_handle")),
        }
    }

    pub fn authenticated_runtime_release_operation(
        &self,
    ) -> Result<&AuthenticatedRuntimeReleaseOperationV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                authenticated_runtime_release_operation,
                ..
            } => Ok(authenticated_runtime_release_operation.as_ref()),
            _ => Err(ContractError::InvalidField(
                "authenticated_runtime_release_operation",
            )),
        }
    }

    pub fn expected_terminal_issuer(&self) -> Result<TerminalReceiptIssuerKey, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                expected_terminal_issuer,
                ..
            } => Ok(*expected_terminal_issuer),
            _ => Err(ContractError::InvalidField("expected_terminal_issuer")),
        }
    }

    pub fn custody_envelope(&self) -> Result<&CustodyEnvelopeV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                custody_envelope, ..
            } => Ok(custody_envelope),
            _ => Err(ContractError::InvalidField("custody_envelope")),
        }
    }

    pub fn media_session_layout(
        &self,
    ) -> Result<&ValidatedCencFmp4MediaSessionLayoutV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                media_session_layout,
                ..
            } => Ok(media_session_layout.as_ref()),
            _ => Err(ContractError::InvalidField("media_session_layout")),
        }
    }

    pub fn protected_init_segment(&self) -> Result<&[u8], ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                protected_init_segment,
                ..
            } => Ok(protected_init_segment.as_slice()),
            _ => Err(ContractError::InvalidField("protected_init_segment")),
        }
    }

    pub fn signed_node_contributions(&self) -> Result<&[SignedNodeContributionV1], ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                signed_node_contributions,
                ..
            } => Ok(signed_node_contributions),
            _ => Err(ContractError::InvalidField("signed_node_contributions")),
        }
    }

    pub fn signed_terminal_receipt(&self) -> Result<&SignedTerminalReceiptV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::OpenViewerSession {
                signed_terminal_receipt,
                ..
            } => Ok(signed_terminal_receipt.as_ref()),
            _ => Err(ContractError::InvalidField("signed_terminal_receipt")),
        }
    }

    pub fn viewer_session_handle(
        &self,
    ) -> Result<&[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::ReadViewerMediaPart {
                viewer_session_handle,
                ..
            }
            | ValidatedDecryptProviderRequestKindV1::CloseViewerSession {
                viewer_session_handle,
                ..
            } => Ok(viewer_session_handle.as_bytes()),
            _ => Err(ContractError::InvalidField("viewer_session_handle")),
        }
    }

    pub fn viewer_media_part_selector(&self) -> Result<&ViewerMediaPartSelectorV1, ContractError> {
        match &self.0 {
            ValidatedDecryptProviderRequestKindV1::ReadViewerMediaPart {
                part_selector, ..
            } => Ok(part_selector),
            _ => Err(ContractError::InvalidField("part_selector")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecryptProviderResponseStatusV1 {
    PreparedRecipient,
    ViewerSessionOpened,
    ViewerMediaPart,
    CancelledPreparedRecipient,
    PreparedRecipientAlreadyAbsent,
    ClosedViewerSession,
    ViewerSessionAlreadyAbsent,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptProviderResponseV1(DecryptProviderResponseKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum DecryptProviderResponseKindV1 {
    PreparedRecipient {
        schema: String,
        audit_request_id: [u8; 32],
        prepared_recipient_handle: OpaqueHandleV1,
        recipient_public_key: Vec<u8>,
        recipient_identity: RecipientIdentityBlobV1,
    },
    ViewerSessionOpened {
        schema: String,
        audit_request_id: [u8; 32],
        viewer_session_handle: OpaqueHandleV1,
    },
    ViewerMediaPart {
        schema: String,
        audit_request_id: [u8; 32],
        viewer_session_handle: OpaqueHandleV1,
        part_selector: ViewerMediaPartSelectorV1,
        clear_media_part: MediaPartBlobV1,
    },
    CancelledPreparedRecipient {
        schema: String,
        audit_request_id: [u8; 32],
        prepared_recipient_handle: OpaqueHandleV1,
    },
    PreparedRecipientAlreadyAbsent {
        schema: String,
        audit_request_id: [u8; 32],
        prepared_recipient_handle: OpaqueHandleV1,
    },
    ClosedViewerSession {
        schema: String,
        audit_request_id: [u8; 32],
        viewer_session_handle: OpaqueHandleV1,
    },
    ViewerSessionAlreadyAbsent {
        schema: String,
        audit_request_id: [u8; 32],
        viewer_session_handle: OpaqueHandleV1,
    },
    Failure {
        schema: String,
        audit_request_id: [u8; 32],
        code: ProviderFailureCodeV1,
    },
}

impl DecryptProviderResponseV1 {
    pub const fn status(&self) -> DecryptProviderResponseStatusV1 {
        match self.0 {
            DecryptProviderResponseKindV1::PreparedRecipient { .. } => {
                DecryptProviderResponseStatusV1::PreparedRecipient
            }
            DecryptProviderResponseKindV1::ViewerSessionOpened { .. } => {
                DecryptProviderResponseStatusV1::ViewerSessionOpened
            }
            DecryptProviderResponseKindV1::ViewerMediaPart { .. } => {
                DecryptProviderResponseStatusV1::ViewerMediaPart
            }
            DecryptProviderResponseKindV1::CancelledPreparedRecipient { .. } => {
                DecryptProviderResponseStatusV1::CancelledPreparedRecipient
            }
            DecryptProviderResponseKindV1::PreparedRecipientAlreadyAbsent { .. } => {
                DecryptProviderResponseStatusV1::PreparedRecipientAlreadyAbsent
            }
            DecryptProviderResponseKindV1::ClosedViewerSession { .. } => {
                DecryptProviderResponseStatusV1::ClosedViewerSession
            }
            DecryptProviderResponseKindV1::ViewerSessionAlreadyAbsent { .. } => {
                DecryptProviderResponseStatusV1::ViewerSessionAlreadyAbsent
            }
            DecryptProviderResponseKindV1::Failure { .. } => {
                DecryptProviderResponseStatusV1::Failure
            }
        }
    }

    pub fn new_prepared_recipient(
        audit_request_id: RuntimeReleaseAuditIdV1,
        prepared_recipient_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        recipient_public_key: RecipientPublicKeyBytesV1,
        recipient_identity: &RecipientKeyIdentityV1,
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::PreparedRecipient {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            prepared_recipient_handle: OpaqueHandleV1::new(prepared_recipient_handle)?,
            recipient_public_key: recipient_public_key.as_bytes().to_vec(),
            recipient_identity: CanonicalBlob::from_contract(recipient_identity)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn new_viewer_session_opened(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::ViewerSessionOpened {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            viewer_session_handle: OpaqueHandleV1::new(viewer_session_handle)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn new_viewer_media_part(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        part_selector: ViewerMediaPartSelectorV1,
        clear_media_part: Vec<u8>,
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::ViewerMediaPart {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            viewer_session_handle: OpaqueHandleV1::new(viewer_session_handle)?,
            part_selector,
            clear_media_part: MediaPartBlobV1::new(clear_media_part)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn new_cancelled_prepared_recipient(
        audit_request_id: RuntimeReleaseAuditIdV1,
        prepared_recipient_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::CancelledPreparedRecipient {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            prepared_recipient_handle: OpaqueHandleV1::new(prepared_recipient_handle)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn new_prepared_recipient_already_absent(
        audit_request_id: RuntimeReleaseAuditIdV1,
        prepared_recipient_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(
            DecryptProviderResponseKindV1::PreparedRecipientAlreadyAbsent {
                schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
                audit_request_id: *audit_request_id.digest().as_bytes(),
                prepared_recipient_handle: OpaqueHandleV1::new(prepared_recipient_handle)?,
            },
        );
        value.validate()?;
        Ok(value)
    }

    pub fn new_closed_viewer_session(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::ClosedViewerSession {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            viewer_session_handle: OpaqueHandleV1::new(viewer_session_handle)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn new_viewer_session_already_absent(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::ViewerSessionAlreadyAbsent {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            viewer_session_handle: OpaqueHandleV1::new(viewer_session_handle)?,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn new_failure(
        audit_request_id: RuntimeReleaseAuditIdV1,
        code: ProviderFailureCodeV1,
    ) -> Result<Self, ContractError> {
        let value = Self(DecryptProviderResponseKindV1::Failure {
            schema: DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            audit_request_id: *audit_request_id.digest().as_bytes(),
            code,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn audit_request_id(&self) -> Result<RuntimeReleaseAuditIdV1, ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::PreparedRecipient {
                audit_request_id, ..
            }
            | DecryptProviderResponseKindV1::ViewerSessionOpened {
                audit_request_id, ..
            }
            | DecryptProviderResponseKindV1::ViewerMediaPart {
                audit_request_id, ..
            }
            | DecryptProviderResponseKindV1::CancelledPreparedRecipient {
                audit_request_id, ..
            }
            | DecryptProviderResponseKindV1::PreparedRecipientAlreadyAbsent {
                audit_request_id,
                ..
            }
            | DecryptProviderResponseKindV1::ClosedViewerSession {
                audit_request_id, ..
            }
            | DecryptProviderResponseKindV1::ViewerSessionAlreadyAbsent {
                audit_request_id, ..
            }
            | DecryptProviderResponseKindV1::Failure {
                audit_request_id, ..
            } => RuntimeReleaseAuditIdV1::new(elastos_protected_content_contracts::Digest32::new(
                *audit_request_id,
            )),
        }
    }

    pub fn prepared_recipient_handle(
        &self,
    ) -> Result<&[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::PreparedRecipient {
                prepared_recipient_handle,
                ..
            }
            | DecryptProviderResponseKindV1::CancelledPreparedRecipient {
                prepared_recipient_handle,
                ..
            }
            | DecryptProviderResponseKindV1::PreparedRecipientAlreadyAbsent {
                prepared_recipient_handle,
                ..
            } => Ok(prepared_recipient_handle.as_bytes()),
            _ => Err(ContractError::InvalidField("prepared_recipient_handle")),
        }
    }

    pub fn recipient_public_key(&self) -> Result<RecipientPublicKeyBytesV1, ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::PreparedRecipient {
                recipient_public_key,
                ..
            } => {
                let bytes: [u8;
                    elastos_protected_content_contracts::PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] =
                    recipient_public_key
                        .as_slice()
                        .try_into()
                        .map_err(|_| ContractError::InvalidField("recipient_public_key"))?;
                RecipientPublicKeyBytesV1::new(bytes)
            }
            _ => Err(ContractError::InvalidField("recipient_public_key")),
        }
    }

    pub fn recipient_identity(&self) -> Result<RecipientKeyIdentityV1, ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::PreparedRecipient {
                recipient_identity, ..
            } => recipient_identity.decode(),
            _ => Err(ContractError::InvalidField("recipient_identity")),
        }
    }

    pub fn viewer_session_handle(
        &self,
    ) -> Result<&[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1], ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::ViewerSessionOpened {
                viewer_session_handle,
                ..
            }
            | DecryptProviderResponseKindV1::ViewerMediaPart {
                viewer_session_handle,
                ..
            }
            | DecryptProviderResponseKindV1::ClosedViewerSession {
                viewer_session_handle,
                ..
            }
            | DecryptProviderResponseKindV1::ViewerSessionAlreadyAbsent {
                viewer_session_handle,
                ..
            } => Ok(viewer_session_handle.as_bytes()),
            _ => Err(ContractError::InvalidField("viewer_session_handle")),
        }
    }

    pub fn failure_code(&self) -> Result<ProviderFailureCodeV1, ContractError> {
        match self.0 {
            DecryptProviderResponseKindV1::Failure { code, .. } => Ok(code),
            _ => Err(ContractError::InvalidField("provider_failure_code")),
        }
    }

    pub fn viewer_media_part_selector(&self) -> Result<&ViewerMediaPartSelectorV1, ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::ViewerMediaPart { part_selector, .. } => {
                Ok(part_selector)
            }
            _ => Err(ContractError::InvalidField("part_selector")),
        }
    }

    pub fn clear_media_part(&self) -> Result<&[u8], ContractError> {
        match &self.0 {
            DecryptProviderResponseKindV1::ViewerMediaPart {
                clear_media_part, ..
            } => Ok(clear_media_part.as_slice()),
            _ => Err(ContractError::InvalidField("clear_media_part")),
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
            DecryptProviderResponseKindV1::PreparedRecipient { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
                    "decrypt_provider_response.schema",
                )?;
                let recipient_public_key = self.recipient_public_key()?;
                let recipient_identity = self.recipient_identity()?;
                if !recipient_identity.matches_public_key(recipient_public_key.as_bytes()) {
                    return Err(ContractError::InvalidField("recipient_identity"));
                }
                self.audit_request_id()?;
                self.prepared_recipient_handle()?;
                Ok(())
            }
            DecryptProviderResponseKindV1::ViewerSessionOpened { schema, .. }
            | DecryptProviderResponseKindV1::ClosedViewerSession { schema, .. }
            | DecryptProviderResponseKindV1::ViewerSessionAlreadyAbsent { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
                    "decrypt_provider_response.schema",
                )?;
                self.audit_request_id()?;
                self.viewer_session_handle()?;
                Ok(())
            }
            DecryptProviderResponseKindV1::ViewerMediaPart { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
                    "decrypt_provider_response.schema",
                )?;
                self.audit_request_id()?;
                self.viewer_session_handle()?;
                let _ = self.viewer_media_part_selector()?;
                let _ = self.clear_media_part()?;
                Ok(())
            }
            DecryptProviderResponseKindV1::CancelledPreparedRecipient { schema, .. }
            | DecryptProviderResponseKindV1::PreparedRecipientAlreadyAbsent { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
                    "decrypt_provider_response.schema",
                )?;
                self.audit_request_id()?;
                self.prepared_recipient_handle()?;
                Ok(())
            }
            DecryptProviderResponseKindV1::Failure { schema, .. } => {
                validate_schema(
                    schema,
                    DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
                    "decrypt_provider_response.schema",
                )?;
                self.audit_request_id()?;
                let _ = self.failure_code()?;
                Ok(())
            }
        }
    }
}

impl Serialize for DecryptProviderResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DecryptProviderResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = DecryptProviderResponseKindV1::deserialize(deserializer)?;
        let value = Self(inner);
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        test_support::{
            custody_envelope_for_media, custody_envelope_for_seed, make_signed_node_contribution,
            make_signed_runtime_release_operation,
            make_signed_runtime_release_operation_for_envelope_and_seed,
            make_signed_terminal_receipt, media_components, media_identity, recipient_identity,
            recipient_public_key, runtime_operation_issuer_for_seed,
        },
        DecryptProviderRequestOpV1, DecryptProviderRequestV1, DecryptProviderResponseStatusV1,
        DecryptProviderResponseV1, ProviderFailureCodeV1, ValidatedDecryptProviderRequestV1,
        ViewerMediaPartSelectorV1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
        MAX_VIEWER_MEDIA_PART_BYTES_V1,
    };
    use elastos_protected_content_contracts::{
        RuntimeReleaseAuditIdV1, SignedRuntimeReleaseOperationV1,
    };

    fn handle(seed: u8) -> [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        let mut bytes = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        bytes[0] = seed.max(1);
        bytes[31] = seed ^ 0x5a;
        bytes
    }

    fn decode_request(
        bytes: &[u8],
    ) -> Result<ValidatedDecryptProviderRequestV1, serde_json::Error> {
        ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            bytes,
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
    }

    fn open_fixture(
        seed: u8,
    ) -> (
        crate::CencFmp4MediaIdentityV1,
        Vec<u8>,
        Vec<Vec<u8>>,
        elastos_protected_content_contracts::CustodyEnvelopeV1,
        SignedRuntimeReleaseOperationV1,
    ) {
        let media_identity = media_identity(seed);
        let (init_segment, encrypted_segments, _, _) = media_components(seed);
        let custody_envelope = custody_envelope_for_media(seed);
        let operation =
            make_signed_runtime_release_operation_for_envelope_and_seed(0x42, &custody_envelope);
        (
            media_identity,
            init_segment,
            encrypted_segments,
            custody_envelope,
            operation,
        )
    }

    #[test]
    fn decrypt_prepare_request_round_trips() {
        let operation = make_signed_runtime_release_operation();
        let request = DecryptProviderRequestV1::new_prepare_recipient(
            operation.statement().release_request().binding(),
            operation.statement().audit_request_id(),
            operation.statement().release_request().action(),
            operation.statement().runtime_operation_issuer(),
            operation.statement().issued_at(),
            operation.statement().expires_at(),
        )
        .unwrap();
        let decoded =
            DecryptProviderRequestV1::decode_wire(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, request);
        let validated = decode_request(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(validated.op(), request.op());
        assert_eq!(
            validated.audit_request_id(),
            operation.statement().audit_request_id()
        );

        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 1,
        )
        .is_err());
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 41,
        )
        .is_err());

        let mut injected_now =
            serde_json::from_slice::<serde_json::Value>(&request.to_json_vec().unwrap()).unwrap();
        injected_now["now_unix_ms"] = serde_json::json!(crate::test_support::NOW + 10);
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &serde_json::to_vec(&injected_now).unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());

        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x43),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn decrypt_open_request_round_trips_and_rejects_wrong_envelope() {
        let (media_identity, init_segment, _, custody_envelope, operation) = open_fixture(0x11);
        let contributions = vec![
            make_signed_node_contribution(&operation, 1),
            make_signed_node_contribution(&operation, 2),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61);
        let request = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &media_identity,
            &init_segment,
            &contributions,
            &terminal,
        )
        .unwrap();
        let decoded =
            DecryptProviderRequestV1::decode_wire(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, request);
        let validated = decode_request(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(validated.op(), request.op());
        assert_eq!(
            validated.audit_request_id(),
            operation.statement().audit_request_id()
        );
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW.saturating_sub(10),
        )
        .is_err());
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 41,
        )
        .is_err());

        let other_operation = make_signed_runtime_release_operation_for_envelope_and_seed(
            0x52,
            &custody_envelope_for_seed(0x52),
        );
        let wrong_contributions = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &media_identity,
            &init_segment,
            &[
                make_signed_node_contribution(&other_operation, 1),
                make_signed_node_contribution(&other_operation, 2),
            ],
            &terminal,
        )
        .unwrap();
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &wrong_contributions.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());

        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x43),
            crate::test_support::NOW + 10,
        )
        .is_err());

        let wrong_media_identity = crate::test_support::media_identity(0x21);
        let wrong_media = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &wrong_media_identity,
            &init_segment,
            &contributions,
            &terminal,
        )
        .unwrap();
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &wrong_media.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());

        let mut wrong_init = init_segment.clone();
        wrong_init[0] ^= 0x01;
        let wrong_init_request = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &media_identity,
            &wrong_init,
            &contributions,
            &terminal,
        )
        .unwrap();
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &wrong_init_request.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn decrypt_open_request_rejects_required_plus_one_duplicate_and_trailing_bytes() {
        let (media_identity, init_segment, _, custody_envelope, operation) = open_fixture(0x11);
        let contributions = vec![
            make_signed_node_contribution(&operation, 1),
            make_signed_node_contribution(&operation, 2),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61);
        let extra_contribution = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &media_identity,
            &init_segment,
            &[
                contributions[0].clone(),
                contributions[1].clone(),
                make_signed_node_contribution(&operation, 3),
            ],
            &terminal,
        )
        .unwrap();
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &extra_contribution.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());

        let duplicate_contribution = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &media_identity,
            &init_segment,
            &[contributions[0].clone(), contributions[0].clone()],
            &terminal,
        )
        .unwrap();
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &duplicate_contribution.to_json_vec().unwrap(),
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());

        let mut trailing = DecryptProviderRequestV1::new_cancel_prepared_recipient(
            operation.statement().audit_request_id(),
            handle(0x21),
        )
        .unwrap()
        .to_json_vec()
        .unwrap();
        trailing.extend_from_slice(br#"{"extra":true}"#);
        assert!(ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            &trailing,
            runtime_operation_issuer_for_seed(0x42),
            crate::test_support::NOW + 10,
        )
        .is_err());
    }

    #[test]
    fn decrypt_response_round_trips_and_redacts_handles() {
        let audit_id = RuntimeReleaseAuditIdV1::new(crate::test_support::digest(0x91)).unwrap();
        let response = DecryptProviderResponseV1::new_prepared_recipient(
            audit_id,
            handle(0x21),
            recipient_public_key(0x30),
            &recipient_identity(0x30),
        )
        .unwrap();
        let decoded =
            DecryptProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, response);

        let debug = format!("{response:?}");
        assert!(!debug.contains("215b"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn decrypt_read_viewer_media_part_contract_is_strict_and_bounded() {
        let (media_identity, init_segment, encrypted_segments, custody_envelope, operation) =
            open_fixture(0x11);
        let contributions = vec![
            make_signed_node_contribution(&operation, 1),
            make_signed_node_contribution(&operation, 2),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61);
        let open_request = DecryptProviderRequestV1::new_open_viewer_session(
            handle(0x21),
            &operation,
            terminal.statement().issuer(),
            &custody_envelope,
            &media_identity,
            &init_segment,
            &contributions,
            &terminal,
        )
        .unwrap();
        let open_validated = decode_request(&open_request.to_json_vec().unwrap()).unwrap();
        assert_eq!(
            open_validated
                .media_session_layout()
                .unwrap()
                .media_identity(),
            &media_identity
        );
        assert_eq!(
            open_validated.protected_init_segment().unwrap(),
            init_segment.as_slice()
        );

        let init_request = DecryptProviderRequestV1::new_read_viewer_media_part(
            operation.statement().audit_request_id(),
            handle(0x41),
            ViewerMediaPartSelectorV1::init(),
        )
        .unwrap();
        let init_json =
            serde_json::from_slice::<serde_json::Value>(&init_request.to_json_vec().unwrap())
                .unwrap();
        assert_eq!(init_json["part_selector"]["part"], "init");
        assert!(init_json["part_selector"]
            .get("encrypted_segment")
            .is_none());
        let validated_init = decode_request(&init_request.to_json_vec().unwrap()).unwrap();
        assert_eq!(
            validated_init.op(),
            DecryptProviderRequestOpV1::ReadViewerMediaPart
        );
        assert!(validated_init
            .viewer_media_part_selector()
            .unwrap()
            .is_init());

        let segment_request = DecryptProviderRequestV1::new_read_viewer_media_part(
            operation.statement().audit_request_id(),
            handle(0x41),
            ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap(),
        )
        .unwrap();
        let validated_segment = decode_request(&segment_request.to_json_vec().unwrap()).unwrap();
        assert_eq!(
            validated_segment
                .viewer_media_part_selector()
                .unwrap()
                .segment_index(),
            Some(1)
        );
        assert_eq!(
            validated_segment
                .viewer_media_part_selector()
                .unwrap()
                .encrypted_segment()
                .unwrap(),
            encrypted_segments[1].as_slice()
        );

        let response = DecryptProviderResponseV1::new_viewer_media_part(
            operation.statement().audit_request_id(),
            handle(0x41),
            ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap(),
            vec![0x10, 0x11, 0x12],
        )
        .unwrap();
        let decoded =
            DecryptProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap()).unwrap();
        assert_eq!(
            decoded.status(),
            DecryptProviderResponseStatusV1::ViewerMediaPart
        );
        assert_eq!(
            decoded
                .viewer_media_part_selector()
                .unwrap()
                .segment_index(),
            Some(1)
        );
        assert_eq!(decoded.clear_media_part().unwrap(), &[0x10, 0x11, 0x12]);

        let mut wrong_shape =
            serde_json::from_slice::<serde_json::Value>(&init_request.to_json_vec().unwrap())
                .unwrap();
        wrong_shape["part_selector"]["encrypted_segment"] = serde_json::json!([1, 2, 3]);
        assert!(decode_request(&serde_json::to_vec(&wrong_shape).unwrap()).is_err());

        let mut missing_segment =
            serde_json::from_slice::<serde_json::Value>(&segment_request.to_json_vec().unwrap())
                .unwrap();
        missing_segment["part_selector"]
            .as_object_mut()
            .unwrap()
            .remove("encrypted_segment");
        assert!(decode_request(&serde_json::to_vec(&missing_segment).unwrap()).is_err());

        let mut oversized =
            serde_json::from_slice::<serde_json::Value>(&segment_request.to_json_vec().unwrap())
                .unwrap();
        oversized["part_selector"]["encrypted_segment"] =
            serde_json::json!(vec![7u8; MAX_VIEWER_MEDIA_PART_BYTES_V1 + 1]);
        assert!(decode_request(&serde_json::to_vec(&oversized).unwrap()).is_err());

        let mut out_of_range =
            serde_json::from_slice::<serde_json::Value>(&segment_request.to_json_vec().unwrap())
                .unwrap();
        out_of_range["part_selector"]["segment_index"] = serde_json::json!(u64::from(u32::MAX) + 1);
        assert!(decode_request(&serde_json::to_vec(&out_of_range).unwrap()).is_err());

        let mut trailing = segment_request.to_json_vec().unwrap();
        trailing.extend_from_slice(br#"{"extra":true}"#);
        assert!(decode_request(&trailing).is_err());

        let debug = format!("{response:?}");
        for forbidden in ["play_url", "route", "credential", "share", "cek"] {
            assert!(!debug.contains(forbidden));
        }
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn decrypt_lifecycle_round_trips_with_exact_binding() {
        let audit_id = RuntimeReleaseAuditIdV1::new(crate::test_support::digest(0x91)).unwrap();
        let (media_identity, init_segment, encrypted_segments, custody_envelope, operation) =
            open_fixture(0x11);
        let contributions = vec![
            make_signed_node_contribution(&operation, 1),
            make_signed_node_contribution(&operation, 2),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61);
        let prepared = handle(0x21);
        let viewer = handle(0x31);

        let requests = [
            DecryptProviderRequestV1::new_prepare_recipient(
                operation.statement().release_request().binding(),
                audit_id,
                operation.statement().release_request().action(),
                operation.statement().runtime_operation_issuer(),
                operation.statement().issued_at(),
                operation.statement().expires_at(),
            )
            .unwrap(),
            DecryptProviderRequestV1::new_open_viewer_session(
                prepared,
                &operation,
                terminal.statement().issuer(),
                &custody_envelope,
                &media_identity,
                &init_segment,
                &contributions,
                &terminal,
            )
            .unwrap(),
            DecryptProviderRequestV1::new_read_viewer_media_part(
                audit_id,
                viewer,
                ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap(),
            )
            .unwrap(),
            DecryptProviderRequestV1::new_cancel_prepared_recipient(audit_id, prepared).unwrap(),
            DecryptProviderRequestV1::new_close_viewer_session(audit_id, viewer).unwrap(),
        ];
        for request in requests {
            let decoded =
                DecryptProviderRequestV1::decode_wire(&request.to_json_vec().unwrap()).unwrap();
            assert_eq!(decoded, request);
            assert_eq!(
                ValidatedDecryptProviderRequestV1::decode_and_validate_at(
                    &request.to_json_vec().unwrap(),
                    runtime_operation_issuer_for_seed(0x42),
                    crate::test_support::NOW + 10,
                )
                .unwrap()
                .op(),
                request.op()
            );
        }

        let responses = [
            DecryptProviderResponseV1::new_prepared_recipient(
                audit_id,
                prepared,
                recipient_public_key(0x30),
                &recipient_identity(0x30),
            )
            .unwrap(),
            DecryptProviderResponseV1::new_viewer_session_opened(audit_id, viewer).unwrap(),
            DecryptProviderResponseV1::new_viewer_media_part(
                audit_id,
                viewer,
                ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap(),
                vec![0x41, 0x42, 0x43],
            )
            .unwrap(),
            DecryptProviderResponseV1::new_cancelled_prepared_recipient(audit_id, prepared)
                .unwrap(),
            DecryptProviderResponseV1::new_prepared_recipient_already_absent(audit_id, prepared)
                .unwrap(),
            DecryptProviderResponseV1::new_closed_viewer_session(audit_id, viewer).unwrap(),
            DecryptProviderResponseV1::new_viewer_session_already_absent(audit_id, viewer).unwrap(),
        ];
        for response in responses {
            let decoded =
                DecryptProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap())
                    .unwrap();
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn decrypt_response_terminal_statuses_and_failure_are_typed() {
        let audit_id = RuntimeReleaseAuditIdV1::new(crate::test_support::digest(0x91)).unwrap();
        let opened =
            DecryptProviderResponseV1::new_viewer_session_opened(audit_id, handle(0x31)).unwrap();
        assert_eq!(
            opened.status(),
            DecryptProviderResponseStatusV1::ViewerSessionOpened
        );
        let media = DecryptProviderResponseV1::new_viewer_media_part(
            audit_id,
            handle(0x31),
            ViewerMediaPartSelectorV1::init(),
            vec![0x41],
        )
        .unwrap();
        assert_eq!(
            media.status(),
            DecryptProviderResponseStatusV1::ViewerMediaPart
        );
        let closed =
            DecryptProviderResponseV1::new_closed_viewer_session(audit_id, handle(0x31)).unwrap();
        assert_eq!(
            closed.status(),
            DecryptProviderResponseStatusV1::ClosedViewerSession
        );
        let absent =
            DecryptProviderResponseV1::new_viewer_session_already_absent(audit_id, handle(0x31))
                .unwrap();
        assert_eq!(
            absent.status(),
            DecryptProviderResponseStatusV1::ViewerSessionAlreadyAbsent
        );
        let failure =
            DecryptProviderResponseV1::new_failure(audit_id, ProviderFailureCodeV1::NotConfigured)
                .unwrap();
        assert_eq!(failure.status(), DecryptProviderResponseStatusV1::Failure);
        assert_eq!(
            failure.failure_code().unwrap(),
            ProviderFailureCodeV1::NotConfigured
        );
    }
}
