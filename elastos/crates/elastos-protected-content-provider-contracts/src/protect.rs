use std::collections::BTreeSet;

use elastos_protected_content_contracts::{
    ContractError, CustodyCommitteeAuthorizationIdentityV1, CustodyEnvelopeV1,
    CustodyEpochIdentityV1, CustodyPoolIdentityV1, Digest32, NodeCustodyPublicKeyV1, NodePublicKey,
    PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::media::{
    validate_visible_ascii, CencFmp4MediaIdentityV1, MAX_CENC_FMP4_MEDIA_IDENTITY_BYTES_V1,
    MAX_MEDIA_DECLARATION_BYTES_V1,
};
use crate::wire::{
    contract_decode_error, decode_json, encode_json, validate_schema, CanonicalBlob,
    OpaqueHandleV1, ProviderFailureCodeV1, MAX_CUSTODY_ENVELOPE_BYTES_V1,
    MAX_PROVIDER_BINDING_BYTES_V1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
};

pub const PROTECT_PROVIDER_REQUEST_SCHEMA_V1: &str =
    "elastos.protected-content.protect-provider.request/v1";
pub const PROTECT_PROVIDER_RESPONSE_SCHEMA_V1: &str =
    "elastos.protected-content.protect-provider.response/v1";
pub const MAX_PROTECT_MEDIA_PART_BYTES_V1: usize = 2 * 1024 * 1024;
pub const MAX_PROTECT_MEDIA_SEGMENTS_V1: u32 = 512;
const REQUIRED_THRESHOLD_REQUIRED_V1: u8 = 2;
const REQUIRED_THRESHOLD_TOTAL_V1: u8 = 3;

type IdentityBlobV1 = CanonicalBlob<MAX_PROVIDER_BINDING_BYTES_V1>;
type MediaIdentityBlobV1 = CanonicalBlob<MAX_CENC_FMP4_MEDIA_IDENTITY_BYTES_V1>;
type CustodyEnvelopeBlobV1 = CanonicalBlob<MAX_CUSTODY_ENVELOPE_BYTES_V1>;
type MediaPartBlobV1 = CanonicalBlob<MAX_PROTECT_MEDIA_PART_BYTES_V1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectProviderRequestOpV1 {
    OpenProtectionSession,
    ProtectMediaSegment,
    FinalizeProtectionSession,
    CancelProtectionSession,
    CloseProtectionSession,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectionSessionNodeV1 {
    node_public_key: [u8; 32],
    node_custody_public_key: Vec<u8>,
}

impl ProtectionSessionNodeV1 {
    pub fn new(
        node_public_key: NodePublicKey,
        node_custody_public_key: NodeCustodyPublicKeyV1,
    ) -> Result<Self, ContractError> {
        let value = Self {
            node_public_key: *node_public_key.as_bytes(),
            node_custody_public_key: node_custody_public_key.as_bytes().to_vec(),
        };
        let _ = value.node_public_key()?;
        let _ = value.node_custody_public_key()?;
        Ok(value)
    }

    pub fn node_public_key(&self) -> Result<NodePublicKey, ContractError> {
        NodePublicKey::new(self.node_public_key)
    }

    pub fn node_custody_public_key(&self) -> Result<NodeCustodyPublicKeyV1, ContractError> {
        let bytes: [u8; PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES] = self
            .node_custody_public_key
            .as_slice()
            .try_into()
            .map_err(|_| ContractError::InvalidField("node_custody_public_key"))?;
        NodeCustodyPublicKeyV1::new(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectProviderRequestV1(ProtectProviderRequestKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ProtectProviderRequestKindV1 {
    OpenProtectionSession {
        schema: String,
        protection_session_request_id: [u8; 32],
        custody_pool: IdentityBlobV1,
        custody_epoch: IdentityBlobV1,
        custody_committee_authorization: IdentityBlobV1,
        threshold_required: u8,
        threshold_total: u8,
        mime_type: String,
        codecs: String,
        segment_count: u32,
        clear_init_segment: MediaPartBlobV1,
        nodes: Vec<ProtectionSessionNodeV1>,
    },
    ProtectMediaSegment {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
        segment_index: u32,
        clear_segment: MediaPartBlobV1,
    },
    FinalizeProtectionSession {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
    },
    CancelProtectionSession {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
    },
    CloseProtectionSession {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
    },
}

impl ProtectProviderRequestV1 {
    pub const fn op(&self) -> ProtectProviderRequestOpV1 {
        match self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { .. } => {
                ProtectProviderRequestOpV1::OpenProtectionSession
            }
            ProtectProviderRequestKindV1::ProtectMediaSegment { .. } => {
                ProtectProviderRequestOpV1::ProtectMediaSegment
            }
            ProtectProviderRequestKindV1::FinalizeProtectionSession { .. } => {
                ProtectProviderRequestOpV1::FinalizeProtectionSession
            }
            ProtectProviderRequestKindV1::CancelProtectionSession { .. } => {
                ProtectProviderRequestOpV1::CancelProtectionSession
            }
            ProtectProviderRequestKindV1::CloseProtectionSession { .. } => {
                ProtectProviderRequestOpV1::CloseProtectionSession
            }
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the producer session must bind the exact committee identities, media declaration, and bounded init bytes"
    )]
    pub fn new_open_protection_session(
        protection_session_request_id: Digest32,
        custody_pool: CustodyPoolIdentityV1,
        custody_epoch: CustodyEpochIdentityV1,
        custody_committee_authorization: CustodyCommitteeAuthorizationIdentityV1,
        mime_type: impl Into<String>,
        codecs: impl Into<String>,
        segment_count: u32,
        clear_init_segment: &[u8],
        nodes: Vec<ProtectionSessionNodeV1>,
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderRequestKindV1::OpenProtectionSession {
            schema: PROTECT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            protection_session_request_id: *protection_session_request_id.as_bytes(),
            custody_pool: CanonicalBlob::from_contract(&custody_pool)?,
            custody_epoch: CanonicalBlob::from_contract(&custody_epoch)?,
            custody_committee_authorization: CanonicalBlob::from_contract(
                &custody_committee_authorization,
            )?,
            threshold_required: REQUIRED_THRESHOLD_REQUIRED_V1,
            threshold_total: REQUIRED_THRESHOLD_TOTAL_V1,
            mime_type: mime_type.into(),
            codecs: codecs.into(),
            segment_count,
            clear_init_segment: MediaPartBlobV1::new(clear_init_segment.to_vec())?,
            nodes,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_protect_media_segment(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        segment_index: u32,
        clear_segment: &[u8],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderRequestKindV1::ProtectMediaSegment {
            schema: PROTECT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
            segment_index,
            clear_segment: MediaPartBlobV1::new(clear_segment.to_vec())?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_finalize_protection_session(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderRequestKindV1::FinalizeProtectionSession {
            schema: PROTECT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_cancel_protection_session(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderRequestKindV1::CancelProtectionSession {
            schema: PROTECT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_close_protection_session(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderRequestKindV1::CloseProtectionSession {
            schema: PROTECT_PROVIDER_REQUEST_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        Self::decode_wire(bytes)
    }

    pub fn protection_session_request_id(&self) -> Option<Digest32> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession {
                protection_session_request_id,
                ..
            } => Some(Digest32::new(*protection_session_request_id)),
            _ => None,
        }
    }

    pub fn custody_pool(&self) -> Result<Option<CustodyPoolIdentityV1>, ContractError> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { custody_pool, .. } => {
                Ok(Some(custody_pool.decode()?))
            }
            _ => Ok(None),
        }
    }

    pub fn custody_epoch(&self) -> Result<Option<CustodyEpochIdentityV1>, ContractError> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { custody_epoch, .. } => {
                Ok(Some(custody_epoch.decode()?))
            }
            _ => Ok(None),
        }
    }

    pub fn custody_committee_authorization(
        &self,
    ) -> Result<Option<CustodyCommitteeAuthorizationIdentityV1>, ContractError> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession {
                custody_committee_authorization,
                ..
            } => Ok(Some(custody_committee_authorization.decode()?)),
            _ => Ok(None),
        }
    }

    pub fn mime_type(&self) -> Option<&str> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { mime_type, .. } => {
                Some(mime_type.as_str())
            }
            _ => None,
        }
    }

    pub fn codecs(&self) -> Option<&str> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { codecs, .. } => {
                Some(codecs.as_str())
            }
            _ => None,
        }
    }

    pub fn segment_count(&self) -> Option<u32> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { segment_count, .. } => {
                Some(*segment_count)
            }
            _ => None,
        }
    }

    pub fn clear_init_segment(&self) -> Option<&[u8]> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession {
                clear_init_segment, ..
            } => Some(clear_init_segment.as_slice()),
            _ => None,
        }
    }

    pub fn nodes(&self) -> Option<&[ProtectionSessionNodeV1]> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession { nodes, .. } => Some(nodes),
            _ => None,
        }
    }

    pub fn protection_session_handle(
        &self,
    ) -> Result<Option<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]>, ContractError> {
        match &self.0 {
            ProtectProviderRequestKindV1::ProtectMediaSegment {
                protection_session_handle,
                ..
            }
            | ProtectProviderRequestKindV1::FinalizeProtectionSession {
                protection_session_handle,
                ..
            }
            | ProtectProviderRequestKindV1::CancelProtectionSession {
                protection_session_handle,
                ..
            }
            | ProtectProviderRequestKindV1::CloseProtectionSession {
                protection_session_handle,
                ..
            } => Ok(Some(*protection_session_handle.as_bytes())),
            _ => Ok(None),
        }
    }

    pub fn segment_index(&self) -> Option<u32> {
        match &self.0 {
            ProtectProviderRequestKindV1::ProtectMediaSegment { segment_index, .. } => {
                Some(*segment_index)
            }
            _ => None,
        }
    }

    pub fn clear_segment(&self) -> Option<&[u8]> {
        match &self.0 {
            ProtectProviderRequestKindV1::ProtectMediaSegment { clear_segment, .. } => {
                Some(clear_segment.as_slice())
            }
            _ => None,
        }
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        match &self.0 {
            ProtectProviderRequestKindV1::OpenProtectionSession {
                schema,
                protection_session_request_id,
                threshold_required,
                threshold_total,
                mime_type,
                codecs,
                segment_count,
                nodes,
                ..
            } => {
                validate_schema(
                    schema,
                    PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
                    "protect_provider_request.schema",
                )?;
                if *protection_session_request_id == [0u8; 32] {
                    return Err(ContractError::InvalidField("protection_session_request_id"));
                }
                if *threshold_required != REQUIRED_THRESHOLD_REQUIRED_V1
                    || *threshold_total != REQUIRED_THRESHOLD_TOTAL_V1
                {
                    return Err(ContractError::InvalidField("threshold"));
                }
                validate_visible_ascii(mime_type, "mime_type", MAX_MEDIA_DECLARATION_BYTES_V1)?;
                validate_visible_ascii(codecs, "codecs", MAX_MEDIA_DECLARATION_BYTES_V1)?;
                if *segment_count == 0 || *segment_count > MAX_PROTECT_MEDIA_SEGMENTS_V1 {
                    return Err(ContractError::InvalidField("segment_count"));
                }
                if nodes.len() != usize::from(REQUIRED_THRESHOLD_TOTAL_V1) {
                    return Err(ContractError::InvalidField("nodes"));
                }
                let mut node_keys = BTreeSet::new();
                let mut custody_keys = BTreeSet::new();
                for node in nodes {
                    let node_public_key = node.node_public_key()?;
                    let node_custody_public_key = node.node_custody_public_key()?;
                    if !node_keys.insert(node_public_key)
                        || !custody_keys.insert(node_custody_public_key)
                    {
                        return Err(ContractError::InvalidField("nodes"));
                    }
                }
                let _ = self
                    .custody_pool()?
                    .ok_or(ContractError::InvalidField("custody_pool"))?;
                let _ = self
                    .custody_epoch()?
                    .ok_or(ContractError::InvalidField("custody_epoch"))?;
                let _ =
                    self.custody_committee_authorization()?
                        .ok_or(ContractError::InvalidField(
                            "custody_committee_authorization",
                        ))?;
                Ok(())
            }
            ProtectProviderRequestKindV1::ProtectMediaSegment { schema, .. }
            | ProtectProviderRequestKindV1::FinalizeProtectionSession { schema, .. }
            | ProtectProviderRequestKindV1::CancelProtectionSession { schema, .. }
            | ProtectProviderRequestKindV1::CloseProtectionSession { schema, .. } => {
                validate_schema(
                    schema,
                    PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
                    "protect_provider_request.schema",
                )
            }
        }
    }

    fn decode_wire(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let inner = decode_json::<ProtectProviderRequestKindV1>(bytes)?;
        let value = Self(inner);
        value.validate_structure().map_err(contract_decode_error)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectProviderResponseStatusV1 {
    ProtectionSessionOpened,
    MediaSegmentProtected,
    ProtectionSessionFinalized,
    ProtectionSessionCancelled,
    ProtectionSessionClosed,
    ProtectionSessionAlreadyAbsent,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectProviderResponseV1(ProtectProviderResponseKindV1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum ProtectProviderResponseKindV1 {
    ProtectionSessionOpened {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
        protected_init_segment: MediaPartBlobV1,
    },
    MediaSegmentProtected {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
        segment_index: u32,
        protected_segment: MediaPartBlobV1,
    },
    ProtectionSessionFinalized {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
        media_identity: MediaIdentityBlobV1,
        custody_envelope: CustodyEnvelopeBlobV1,
    },
    ProtectionSessionCancelled {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
    },
    ProtectionSessionClosed {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
    },
    ProtectionSessionAlreadyAbsent {
        schema: String,
        protection_session_handle: OpaqueHandleV1,
    },
    Failure {
        schema: String,
        failure_code: ProviderFailureCodeV1,
    },
}

impl ProtectProviderResponseV1 {
    pub const fn status(&self) -> ProtectProviderResponseStatusV1 {
        match self.0 {
            ProtectProviderResponseKindV1::ProtectionSessionOpened { .. } => {
                ProtectProviderResponseStatusV1::ProtectionSessionOpened
            }
            ProtectProviderResponseKindV1::MediaSegmentProtected { .. } => {
                ProtectProviderResponseStatusV1::MediaSegmentProtected
            }
            ProtectProviderResponseKindV1::ProtectionSessionFinalized { .. } => {
                ProtectProviderResponseStatusV1::ProtectionSessionFinalized
            }
            ProtectProviderResponseKindV1::ProtectionSessionCancelled { .. } => {
                ProtectProviderResponseStatusV1::ProtectionSessionCancelled
            }
            ProtectProviderResponseKindV1::ProtectionSessionClosed { .. } => {
                ProtectProviderResponseStatusV1::ProtectionSessionClosed
            }
            ProtectProviderResponseKindV1::ProtectionSessionAlreadyAbsent { .. } => {
                ProtectProviderResponseStatusV1::ProtectionSessionAlreadyAbsent
            }
            ProtectProviderResponseKindV1::Failure { .. } => {
                ProtectProviderResponseStatusV1::Failure
            }
        }
    }

    pub fn new_opened(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        protected_init_segment: &[u8],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderResponseKindV1::ProtectionSessionOpened {
            schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
            protected_init_segment: MediaPartBlobV1::new(protected_init_segment.to_vec())?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_segment_protected(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        segment_index: u32,
        protected_segment: &[u8],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderResponseKindV1::MediaSegmentProtected {
            schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
            segment_index,
            protected_segment: MediaPartBlobV1::new(protected_segment.to_vec())?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_finalized(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
        media_identity: &CencFmp4MediaIdentityV1,
        custody_envelope: &CustodyEnvelopeV1,
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderResponseKindV1::ProtectionSessionFinalized {
            schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
            media_identity: CanonicalBlob::from_contract(media_identity)?,
            custody_envelope: CanonicalBlob::from_contract(custody_envelope)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_cancelled(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderResponseKindV1::ProtectionSessionCancelled {
            schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_closed(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderResponseKindV1::ProtectionSessionClosed {
            schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_already_absent(
        protection_session_handle: [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1],
    ) -> Result<Self, ContractError> {
        let value = Self(
            ProtectProviderResponseKindV1::ProtectionSessionAlreadyAbsent {
                schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
                protection_session_handle: OpaqueHandleV1::new(protection_session_handle)?,
            },
        );
        value.validate_structure()?;
        Ok(value)
    }

    pub fn new_failure(failure_code: ProviderFailureCodeV1) -> Result<Self, ContractError> {
        let value = Self(ProtectProviderResponseKindV1::Failure {
            schema: PROTECT_PROVIDER_RESPONSE_SCHEMA_V1.to_string(),
            failure_code,
        });
        value.validate_structure()?;
        Ok(value)
    }

    pub fn to_json_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        encode_json(self)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let inner = decode_json::<ProtectProviderResponseKindV1>(bytes)?;
        let value = Self(inner);
        value.validate_structure().map_err(contract_decode_error)?;
        Ok(value)
    }

    pub fn protection_session_handle(
        &self,
    ) -> Result<Option<[u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1]>, ContractError> {
        match &self.0 {
            ProtectProviderResponseKindV1::ProtectionSessionOpened {
                protection_session_handle,
                ..
            }
            | ProtectProviderResponseKindV1::MediaSegmentProtected {
                protection_session_handle,
                ..
            }
            | ProtectProviderResponseKindV1::ProtectionSessionFinalized {
                protection_session_handle,
                ..
            }
            | ProtectProviderResponseKindV1::ProtectionSessionCancelled {
                protection_session_handle,
                ..
            }
            | ProtectProviderResponseKindV1::ProtectionSessionClosed {
                protection_session_handle,
                ..
            }
            | ProtectProviderResponseKindV1::ProtectionSessionAlreadyAbsent {
                protection_session_handle,
                ..
            } => Ok(Some(*protection_session_handle.as_bytes())),
            ProtectProviderResponseKindV1::Failure { .. } => Ok(None),
        }
    }

    pub fn protected_init_segment(&self) -> Option<&[u8]> {
        match &self.0 {
            ProtectProviderResponseKindV1::ProtectionSessionOpened {
                protected_init_segment,
                ..
            } => Some(protected_init_segment.as_slice()),
            _ => None,
        }
    }

    pub fn segment_index(&self) -> Option<u32> {
        match &self.0 {
            ProtectProviderResponseKindV1::MediaSegmentProtected { segment_index, .. } => {
                Some(*segment_index)
            }
            _ => None,
        }
    }

    pub fn protected_segment(&self) -> Option<&[u8]> {
        match &self.0 {
            ProtectProviderResponseKindV1::MediaSegmentProtected {
                protected_segment, ..
            } => Some(protected_segment.as_slice()),
            _ => None,
        }
    }

    pub fn media_identity(&self) -> Result<Option<CencFmp4MediaIdentityV1>, ContractError> {
        match &self.0 {
            ProtectProviderResponseKindV1::ProtectionSessionFinalized {
                media_identity, ..
            } => Ok(Some(media_identity.decode()?)),
            _ => Ok(None),
        }
    }

    pub fn custody_envelope(&self) -> Result<Option<CustodyEnvelopeV1>, ContractError> {
        match &self.0 {
            ProtectProviderResponseKindV1::ProtectionSessionFinalized {
                custody_envelope, ..
            } => Ok(Some(custody_envelope.decode()?)),
            _ => Ok(None),
        }
    }

    pub const fn failure_code(&self) -> Option<ProviderFailureCodeV1> {
        match self.0 {
            ProtectProviderResponseKindV1::Failure { failure_code, .. } => Some(failure_code),
            _ => None,
        }
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        match &self.0 {
            ProtectProviderResponseKindV1::ProtectionSessionOpened { schema, .. }
            | ProtectProviderResponseKindV1::MediaSegmentProtected { schema, .. }
            | ProtectProviderResponseKindV1::ProtectionSessionFinalized { schema, .. }
            | ProtectProviderResponseKindV1::ProtectionSessionCancelled { schema, .. }
            | ProtectProviderResponseKindV1::ProtectionSessionClosed { schema, .. }
            | ProtectProviderResponseKindV1::ProtectionSessionAlreadyAbsent { schema, .. }
            | ProtectProviderResponseKindV1::Failure { schema, .. } => validate_schema(
                schema,
                PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
                "protect_provider_response.schema",
            ),
        }?;
        if let ProtectProviderResponseKindV1::ProtectionSessionFinalized {
            media_identity,
            custody_envelope,
            ..
        } = &self.0
        {
            let media_identity = media_identity.decode::<CencFmp4MediaIdentityV1>()?;
            let custody_envelope = custody_envelope.decode::<CustodyEnvelopeV1>()?;
            if custody_envelope.manifest().encrypted_content() != media_identity.encrypted_content()
            {
                return Err(ContractError::InvalidField("custody_envelope"));
            }
        }
        Ok(())
    }
}

impl Serialize for ProtectProviderRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProtectProviderRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = ProtectProviderRequestKindV1::deserialize(deserializer)?;
        let value = Self(inner);
        value.validate_structure().map_err(D::Error::custom)?;
        Ok(value)
    }
}

impl Serialize for ProtectProviderResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProtectProviderResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = ProtectProviderResponseKindV1::deserialize(deserializer)?;
        let value = Self(inner);
        value.validate_structure().map_err(D::Error::custom)?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::{
        custody_envelope_for_media, digest, media_components, media_identity,
        node_custody_public_key, node_public_key,
    };

    use super::*;

    fn handle(seed: u8) -> [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1] {
        let mut out = [0u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
        out[0] = seed.max(1);
        out[31] = seed ^ 0x5a;
        out
    }

    fn nodes() -> Vec<ProtectionSessionNodeV1> {
        [1u8, 2, 3]
            .into_iter()
            .map(|seed| {
                ProtectionSessionNodeV1::new(node_public_key(seed), node_custody_public_key(seed))
                    .unwrap()
            })
            .collect()
    }

    fn custody_pool_identity() -> CustodyPoolIdentityV1 {
        CustodyPoolIdentityV1::new(digest(0x35), 512).unwrap()
    }

    fn custody_epoch_identity() -> CustodyEpochIdentityV1 {
        CustodyEpochIdentityV1::new(digest(0x33), 512).unwrap()
    }

    fn custody_committee_authorization_identity() -> CustodyCommitteeAuthorizationIdentityV1 {
        CustodyCommitteeAuthorizationIdentityV1::new(digest(0x36), 512).unwrap()
    }

    fn open_request_with(
        mime_type: &str,
        codecs: &str,
        segment_count: u32,
        clear_init_segment: &[u8],
    ) -> Result<ProtectProviderRequestV1, ContractError> {
        ProtectProviderRequestV1::new_open_protection_session(
            digest(0xa1),
            custody_pool_identity(),
            custody_epoch_identity(),
            custody_committee_authorization_identity(),
            mime_type,
            codecs,
            segment_count,
            clear_init_segment,
            nodes(),
        )
    }

    #[test]
    fn open_request_round_trips_and_enforces_bounds() {
        let (init_segment, _, _, _) = media_components(0x11);
        let request =
            open_request_with("video/mp4", "avc1.640028,mp4a.40.2", 2, &init_segment).unwrap();

        let decoded =
            ProtectProviderRequestV1::from_json_slice(&request.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(decoded.segment_count(), Some(2));
        assert_eq!(decoded.clear_init_segment(), Some(init_segment.as_slice()));
    }

    #[test]
    fn open_request_rejects_bad_threshold_and_duplicate_nodes() {
        let (init_segment, _, _, _) = media_components(0x11);
        let mut value = serde_json::from_slice::<serde_json::Value>(
            &open_request_with("video/mp4", "avc1.640028,mp4a.40.2", 2, &init_segment)
                .unwrap()
                .to_json_vec()
                .unwrap(),
        )
        .unwrap();
        value["threshold_required"] = serde_json::json!(1);
        assert!(
            ProtectProviderRequestV1::from_json_slice(&serde_json::to_vec(&value).unwrap())
                .is_err()
        );

        let mut dup = serde_json::from_slice::<serde_json::Value>(
            &open_request_with("video/mp4", "avc1.640028,mp4a.40.2", 2, &init_segment)
                .unwrap()
                .to_json_vec()
                .unwrap(),
        )
        .unwrap();
        dup["nodes"][1]["node_public_key"] = dup["nodes"][0]["node_public_key"].clone();
        assert!(
            ProtectProviderRequestV1::from_json_slice(&serde_json::to_vec(&dup).unwrap()).is_err()
        );
    }

    #[test]
    fn open_request_rejects_non_visible_or_oversize_media_declaration() {
        let (init_segment, _, _, _) = media_components(0x11);
        let too_long = "a".repeat(MAX_MEDIA_DECLARATION_BYTES_V1 + 1);

        assert!(open_request_with(&too_long, "avc1.640028,mp4a.40.2", 2, &init_segment).is_err());
        assert!(open_request_with("video/mp4", &too_long, 2, &init_segment).is_err());
        assert!(
            open_request_with("video/mp4\n", "avc1.640028,mp4a.40.2", 2, &init_segment).is_err()
        );
        assert!(
            open_request_with("video/mp4", "avc1.640028, mp4a.40.2", 2, &init_segment).is_err()
        );
    }

    #[test]
    fn protect_wire_rejects_zero_ids_handles_counts_parts_and_unknown_fields() {
        let (init_segment, _, _, _) = media_components(0x11);

        assert!(open_request_with("video/mp4", "avc1.640028,mp4a.40.2", 0, &init_segment).is_err());
        assert!(open_request_with(
            "video/mp4",
            "avc1.640028,mp4a.40.2",
            MAX_PROTECT_MEDIA_SEGMENTS_V1 + 1,
            &init_segment,
        )
        .is_err());
        assert!(open_request_with("video/mp4", "avc1.640028,mp4a.40.2", 2, &[]).is_err());
        assert!(open_request_with(
            "video/mp4",
            "avc1.640028,mp4a.40.2",
            2,
            &vec![0u8; MAX_PROTECT_MEDIA_PART_BYTES_V1 + 1],
        )
        .is_err());
        assert!(ProtectProviderRequestV1::new_open_protection_session(
            Digest32::new([0u8; 32]),
            custody_pool_identity(),
            custody_epoch_identity(),
            custody_committee_authorization_identity(),
            "video/mp4",
            "avc1.640028,mp4a.40.2",
            2,
            &init_segment,
            nodes(),
        )
        .is_err());
        assert!(ProtectProviderRequestV1::new_protect_media_segment([0u8; 32], 0, b"x").is_err());
        assert!(ProtectProviderRequestV1::new_protect_media_segment(
            handle(0x22),
            0,
            &vec![0u8; MAX_PROTECT_MEDIA_PART_BYTES_V1 + 1],
        )
        .is_err());
        assert!(ProtectProviderRequestV1::new_protect_media_segment(handle(0x22), 0, &[]).is_err());

        let mut request_value = serde_json::from_slice::<serde_json::Value>(
            &open_request_with("video/mp4", "avc1.640028,mp4a.40.2", 2, &init_segment)
                .unwrap()
                .to_json_vec()
                .unwrap(),
        )
        .unwrap();
        request_value["unexpected"] = serde_json::json!(true);
        assert!(ProtectProviderRequestV1::from_json_slice(
            &serde_json::to_vec(&request_value).unwrap()
        )
        .is_err());

        let mut response_value = serde_json::from_slice::<serde_json::Value>(
            &ProtectProviderResponseV1::new_opened(handle(0x23), b"protected-init")
                .unwrap()
                .to_json_vec()
                .unwrap(),
        )
        .unwrap();
        response_value["unexpected"] = serde_json::json!(true);
        assert!(ProtectProviderResponseV1::from_json_slice(
            &serde_json::to_vec(&response_value).unwrap()
        )
        .is_err());
    }

    #[test]
    fn maximum_media_part_requests_stay_within_provider_frame_limit() {
        let max_decl = "a".repeat(MAX_MEDIA_DECLARATION_BYTES_V1);
        let open_request = open_request_with(
            &max_decl,
            &max_decl,
            MAX_PROTECT_MEDIA_SEGMENTS_V1,
            &vec![0x55; MAX_PROTECT_MEDIA_PART_BYTES_V1],
        )
        .unwrap();
        let open_json = open_request.to_json_vec().unwrap();
        assert!(open_json.len() <= crate::MAX_PROVIDER_FRAME_BYTES_V1);

        let segment_request = ProtectProviderRequestV1::new_protect_media_segment(
            handle(0x44),
            MAX_PROTECT_MEDIA_SEGMENTS_V1 - 1,
            &vec![0x66; MAX_PROTECT_MEDIA_PART_BYTES_V1],
        )
        .unwrap();
        let segment_json = segment_request.to_json_vec().unwrap();
        assert!(segment_json.len() <= crate::MAX_PROVIDER_FRAME_BYTES_V1);
    }

    #[test]
    fn finalized_response_round_trips_and_binds_media_to_envelope() {
        let response = ProtectProviderResponseV1::new_finalized(
            handle(0x33),
            &media_identity(0x11),
            &custody_envelope_for_media(0x11),
        )
        .unwrap();
        let decoded =
            ProtectProviderResponseV1::from_json_slice(&response.to_json_vec().unwrap()).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn finalized_response_keeps_sealed_share_bytes_private_to_the_wire_surface() {
        let response = ProtectProviderResponseV1::new_finalized(
            handle(0x33),
            &media_identity(0x11),
            &custody_envelope_for_media(0x11),
        )
        .unwrap();

        let json = String::from_utf8(response.to_json_vec().unwrap()).unwrap();
        let debug = format!("{response:?}");
        for term in [
            "content_key",
            "raw_share",
            "plaintext_share",
            "recipient_secret",
        ] {
            assert!(!json.contains(term));
            assert!(!debug.contains(term));
        }
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn finalized_response_rejects_mismatched_envelope() {
        let mut value = serde_json::from_slice::<serde_json::Value>(
            &ProtectProviderResponseV1::new_finalized(
                handle(0x33),
                &media_identity(0x11),
                &custody_envelope_for_media(0x11),
            )
            .unwrap()
            .to_json_vec()
            .unwrap(),
        )
        .unwrap();
        value["custody_envelope"] = serde_json::json!(
            CanonicalBlob::<MAX_CUSTODY_ENVELOPE_BYTES_V1>::from_contract(
                &custody_envelope_for_media(0x21)
            )
            .unwrap()
            .as_slice()
        );
        assert!(
            ProtectProviderResponseV1::from_json_slice(&serde_json::to_vec(&value).unwrap())
                .is_err()
        );
    }
}
