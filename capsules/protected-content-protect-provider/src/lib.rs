use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};

use elastos_protected_content_contracts::{
    CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIdentityV1, CustodyPoolIdentityV1,
};
use elastos_protected_content_custody::{
    protect_validated_clear_fmp4_init_to_cenc_v1, protect_validated_clear_fmp4_segment_to_cenc_v1,
    provision_custody_envelope_for_exact_nodes, ContentEncryptionKeyV1, ExactCustodyEnvelopeNodeV1,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, ProtectProviderRequestOpV1, ProtectProviderRequestV1,
    ProtectProviderResponseV1, ProtectionSessionNodeV1, ProviderFailureCodeV1,
    ValidatedClearFmp4MediaSessionLayoutV1, MAX_PROVIDER_FRAME_BYTES_V1,
    MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1, PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
    PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const INIT_ERROR_CODE: &str = "invalid_config";
const REQUEST_ERROR_CODE: &str = "invalid_request";
const MAX_ACTIVE_SESSIONS_V1: usize = 64;
const MAX_TERMINAL_REPLAYS_V1: usize = 64;
const MAX_AGGREGATE_PROTECTED_MEDIA_BYTES_V1: usize = 16 * 1024 * 1024;

type HandleBytes = [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
type RequestDigest = [u8; 32];

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlRequest {
    Init { config: Value },
    Status,
    Shutdown,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProviderResponse {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: &'static str,
        message: &'static str,
    },
}

impl ProviderResponse {
    fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &'static str, message: &'static str) -> Self {
        Self::Error { code, message }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitConfig {
    #[serde(default)]
    base_path: String,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    encryption_key: String,
    extra: ProtectInitExtraConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectInitExtraConfig {}

#[derive(Clone)]
struct OpenReplayEntry {
    request_digest: RequestDigest,
    response: ProtectProviderResponseV1,
}

#[derive(Clone)]
struct SegmentReplayEntry {
    request_digest: RequestDigest,
}

struct ProtectionSessionEntry {
    custody_pool: CustodyPoolIdentityV1,
    custody_epoch: CustodyEpochIdentityV1,
    custody_committee_authorization: CustodyCommitteeAuthorizationIdentityV1,
    nodes: Vec<ExactCustodyEnvelopeNodeV1>,
    mime_type: String,
    codecs: String,
    clear_session_layout: ValidatedClearFmp4MediaSessionLayoutV1,
    protected_init_segment: Vec<u8>,
    protected_segments: Vec<Vec<u8>>,
    segment_replays: BTreeMap<u32, SegmentReplayEntry>,
    content_key: Option<ContentEncryptionKeyV1>,
    iv_prefix: [u8; 4],
    next_iv_counter: u32,
    segment_count: u32,
    next_segment_index: u32,
    aggregate_protected_bytes: usize,
    finalized: Option<ProtectProviderResponseV1>,
}

struct ConfiguredProtectProvider {
    open_replays: BTreeMap<[u8; 32], OpenReplayEntry>,
    sessions: BTreeMap<HandleBytes, ProtectionSessionEntry>,
    closed_handles: BTreeSet<HandleBytes>,
}

pub struct ProtectProvider {
    state: Option<ConfiguredProtectProvider>,
}

impl Default for ProtectProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProtectProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.state {
            Some(state) => formatter
                .debug_struct("ProtectProvider")
                .field("configured", &true)
                .field("open_replays", &state.open_replays.len())
                .field("active_sessions", &state.sessions.len())
                .field("closed_handles", &state.closed_handles.len())
                .finish(),
            None => formatter
                .debug_struct("ProtectProvider")
                .field("configured", &false)
                .finish(),
        }
    }
}

impl ProtectProvider {
    pub fn new() -> Self {
        Self { state: None }
    }

    pub fn handle_frame(&mut self, frame: &[u8]) -> (ProviderResponse, bool) {
        let mut value = match serde_json::from_slice::<Value>(frame) {
            Ok(value) => value,
            Err(_) => return (invalid_request(), false),
        };
        let envelope = match strip_runtime_invocation_envelope(&mut value, "protect") {
            Ok(state) => state,
            Err(()) => return (invalid_request(), false),
        };
        let op = value.get("op").and_then(Value::as_str).map(str::to_owned);
        match op.as_deref() {
            Some("init" | "status" | "shutdown") => {
                if !control_request_has_exact_fields(&value, op.as_deref().unwrap_or_default()) {
                    return (invalid_request(), false);
                }
                match serde_json::from_value::<ControlRequest>(value) {
                    Ok(ControlRequest::Init { config }) => (self.init(config), false),
                    Ok(ControlRequest::Status) => (self.status(), false),
                    Ok(ControlRequest::Shutdown) => (ProviderResponse::empty_ok(), true),
                    Err(_) => (invalid_request(), false),
                }
            }
            Some(
                "open_protection_session"
                | "protect_media_segment"
                | "finalize_protection_session"
                | "cancel_protection_session"
                | "close_protection_session",
            ) => {
                if !matches!(envelope, EnvelopeState::Present) {
                    return (invalid_request(), false);
                }
                let Ok(bytes) = serde_json::to_vec(&value) else {
                    return (invalid_request(), false);
                };
                (self.handle_protect_request(&bytes), false)
            }
            _ => (invalid_request(), false),
        }
    }

    fn init(&mut self, config: Value) -> ProviderResponse {
        self.state = None;
        match load_provider_state(config) {
            Ok(state) => {
                self.state = Some(state);
                self.status()
            }
            Err(()) => ProviderResponse::error(
                INIT_ERROR_CODE,
                "protect provider configuration is invalid",
            ),
        }
    }

    fn status(&self) -> ProviderResponse {
        ProviderResponse::ok(json!({
            "provider": "protected-content-protect",
            "version": PROVIDER_VERSION,
            "configured": self.state.is_some(),
            "request_schema": PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
            "response_schema": PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
            "supported_operations": [
                "status",
                "open_protection_session",
                "protect_media_segment",
                "finalize_protection_session",
                "cancel_protection_session",
                "close_protection_session",
                "shutdown"
            ],
        }))
    }

    fn handle_protect_request(&mut self, request_bytes: &[u8]) -> ProviderResponse {
        let Some(state) = self.state.as_mut() else {
            return typed_response(ProtectProviderResponseV1::new_failure(
                ProviderFailureCodeV1::NotConfigured,
            ));
        };
        let request = match ProtectProviderRequestV1::from_json_slice(request_bytes) {
            Ok(value) => value,
            Err(_) => {
                return typed_response(ProtectProviderResponseV1::new_failure(
                    ProviderFailureCodeV1::InvalidRequest,
                ));
            }
        };
        let request_digest = sha256(request_bytes);
        match request.op() {
            ProtectProviderRequestOpV1::OpenProtectionSession => {
                let (
                    Some(session_request_id),
                    Ok(Some(custody_pool)),
                    Ok(Some(custody_epoch)),
                    Ok(Some(custody_committee_authorization)),
                    Some(mime_type),
                    Some(codecs),
                    Some(segment_count),
                ) = (
                    request.protection_session_request_id(),
                    request.custody_pool(),
                    request.custody_epoch(),
                    request.custody_committee_authorization(),
                    request.mime_type(),
                    request.codecs(),
                    request.segment_count(),
                )
                else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                let mime_type = mime_type.to_string();
                let codecs = codecs.to_string();
                let request_id = *session_request_id.as_bytes();
                if let Some(replay) = state.open_replays.get(&request_id) {
                    return if replay.request_digest == request_digest {
                        typed_ok(replay.response.clone())
                    } else {
                        typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::BindingMismatch,
                        ))
                    };
                }
                if state.sessions.len() >= MAX_ACTIVE_SESSIONS_V1 {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::BackendUnavailable,
                    ));
                }
                let Some(clear_init_segment) = request.clear_init_segment() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                let clear_session_layout =
                    match ValidatedClearFmp4MediaSessionLayoutV1::new(clear_init_segment) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(ProtectProviderResponseV1::new_failure(
                                ProviderFailureCodeV1::InvalidRequest,
                            ));
                        }
                    };
                let content_key = match ContentEncryptionKeyV1::generate() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let key_id = match request.content_access_id() {
                    Ok(Some(value)) => *value.as_bytes(),
                    _ => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InvalidRequest,
                        ));
                    }
                };
                let iv_prefix = match random_bytes::<4>() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let protected_init_segment = match protect_validated_clear_fmp4_init_to_cenc_v1(
                    &clear_session_layout,
                    clear_init_segment,
                    key_id,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InvalidRequest,
                        ));
                    }
                };
                if protected_init_segment.len() > MAX_AGGREGATE_PROTECTED_MEDIA_BYTES_V1 {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                }
                let Some(request_nodes) = request.nodes() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                let nodes = match request_nodes
                    .iter()
                    .map(exact_custody_node)
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InvalidRequest,
                        ));
                    }
                };
                let handle =
                    match next_unique_handle(state.sessions.keys(), state.closed_handles.iter()) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(ProtectProviderResponseV1::new_failure(
                                ProviderFailureCodeV1::BackendUnavailable,
                            ));
                        }
                    };
                let response =
                    match ProtectProviderResponseV1::new_opened(handle, &protected_init_segment) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(ProtectProviderResponseV1::new_failure(
                                ProviderFailureCodeV1::InternalFailure,
                            ));
                        }
                    };
                let aggregate_protected_bytes = match response.protected_init_segment() {
                    Some(protected_init) => protected_init.len(),
                    None => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                trim_btree_map(&mut state.open_replays, MAX_TERMINAL_REPLAYS_V1);
                state.open_replays.insert(
                    request_id,
                    OpenReplayEntry {
                        request_digest,
                        response: response.clone(),
                    },
                );
                state.sessions.insert(
                    handle,
                    ProtectionSessionEntry {
                        custody_pool,
                        custody_epoch,
                        custody_committee_authorization,
                        nodes,
                        mime_type,
                        codecs,
                        clear_session_layout,
                        protected_init_segment,
                        protected_segments: Vec::with_capacity(segment_count as usize),
                        segment_replays: BTreeMap::new(),
                        content_key: Some(content_key),
                        iv_prefix,
                        next_iv_counter: 0,
                        segment_count,
                        next_segment_index: 0,
                        aggregate_protected_bytes,
                        finalized: None,
                    },
                );
                typed_ok(response)
            }
            ProtectProviderRequestOpV1::ProtectMediaSegment => {
                let Ok(Some(handle)) = request.protection_session_handle() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                let Some(session) = state.sessions.get_mut(&handle) else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::HandleAbsent,
                    ));
                };
                let Some(segment_index) = request.segment_index() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                if let Some(replay) = session.segment_replays.get(&segment_index) {
                    return if replay.request_digest == request_digest {
                        match session.protected_segments.get(segment_index as usize) {
                            Some(protected_segment) => {
                                match ProtectProviderResponseV1::new_segment_protected(
                                    handle,
                                    segment_index,
                                    protected_segment,
                                ) {
                                    Ok(response) => typed_ok(response),
                                    Err(_) => {
                                        typed_response(ProtectProviderResponseV1::new_failure(
                                            ProviderFailureCodeV1::InternalFailure,
                                        ))
                                    }
                                }
                            }
                            None => typed_response(ProtectProviderResponseV1::new_failure(
                                ProviderFailureCodeV1::InternalFailure,
                            )),
                        }
                    } else {
                        typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::BindingMismatch,
                        ))
                    };
                }
                if session.finalized.is_some()
                    || segment_index != session.next_segment_index
                    || segment_index >= session.segment_count
                {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                }
                let Some(content_key) = session.content_key.as_ref() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::BindingMismatch,
                    ));
                };
                let Some(clear_segment) = request.clear_segment() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                let clear_segment_layout =
                    match session.clear_session_layout.validate_segment(clear_segment) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(ProtectProviderResponseV1::new_failure(
                                ProviderFailureCodeV1::InvalidRequest,
                            ));
                        }
                    };
                let sample_count = clear_segment_layout.samples().len();
                let sample_ivs = match allocate_sample_ivs(
                    session.iv_prefix,
                    &mut session.next_iv_counter,
                    sample_count,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::BackendUnavailable,
                        ));
                    }
                };
                let protected_segment = match protect_validated_clear_fmp4_segment_to_cenc_v1(
                    &clear_segment_layout,
                    clear_segment,
                    content_key,
                    &sample_ivs,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InvalidRequest,
                        ));
                    }
                };
                let new_aggregate = match session
                    .aggregate_protected_bytes
                    .checked_add(protected_segment.len())
                {
                    Some(value) if value <= MAX_AGGREGATE_PROTECTED_MEDIA_BYTES_V1 => value,
                    _ => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InvalidRequest,
                        ));
                    }
                };
                let response = match ProtectProviderResponseV1::new_segment_protected(
                    handle,
                    segment_index,
                    &protected_segment,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                session.aggregate_protected_bytes = new_aggregate;
                session.next_segment_index = session.next_segment_index.saturating_add(1);
                session.protected_segments.push(protected_segment);
                session
                    .segment_replays
                    .insert(segment_index, SegmentReplayEntry { request_digest });
                typed_ok(response)
            }
            ProtectProviderRequestOpV1::FinalizeProtectionSession => {
                let Ok(Some(handle)) = request.protection_session_handle() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                let Some(session) = state.sessions.get_mut(&handle) else {
                    return typed_response(if state.closed_handles.contains(&handle) {
                        ProtectProviderResponseV1::new_already_absent(handle)
                    } else {
                        ProtectProviderResponseV1::new_failure(ProviderFailureCodeV1::HandleAbsent)
                    });
                };
                if let Some(response) = &session.finalized {
                    return typed_ok(response.clone());
                }
                if session.next_segment_index != session.segment_count {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                }
                let Some(content_key) = session.content_key.as_ref() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::BindingMismatch,
                    ));
                };
                let media_identity = match CencFmp4MediaIdentityV1::new_from_bytes(
                    &session.protected_init_segment,
                    &session.protected_segments,
                    &session.mime_type,
                    &session.codecs,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let custody_envelope = match provision_custody_envelope_for_exact_nodes(
                    media_identity.encrypted_content().clone(),
                    content_key,
                    session.custody_pool,
                    session.custody_epoch,
                    session.custody_committee_authorization,
                    &session.nodes,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let response = match ProtectProviderResponseV1::new_finalized(
                    handle,
                    &media_identity,
                    &custody_envelope,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(ProtectProviderResponseV1::new_failure(
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                session.finalized = Some(response.clone());
                session.content_key = None;
                typed_ok(response)
            }
            ProtectProviderRequestOpV1::CancelProtectionSession => {
                let Ok(Some(handle)) = request.protection_session_handle() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                if state.sessions.remove(&handle).is_some() {
                    trim_btree_set(&mut state.closed_handles, MAX_TERMINAL_REPLAYS_V1);
                    state.closed_handles.insert(handle);
                    typed_response(ProtectProviderResponseV1::new_cancelled(handle))
                } else {
                    typed_response(ProtectProviderResponseV1::new_already_absent(handle))
                }
            }
            ProtectProviderRequestOpV1::CloseProtectionSession => {
                let Ok(Some(handle)) = request.protection_session_handle() else {
                    return typed_response(ProtectProviderResponseV1::new_failure(
                        ProviderFailureCodeV1::InvalidRequest,
                    ));
                };
                if state.sessions.remove(&handle).is_some() {
                    trim_btree_set(&mut state.closed_handles, MAX_TERMINAL_REPLAYS_V1);
                    state.closed_handles.insert(handle);
                    typed_response(ProtectProviderResponseV1::new_closed(handle))
                } else {
                    typed_response(ProtectProviderResponseV1::new_already_absent(handle))
                }
            }
        }
    }
}

fn exact_custody_node(node: &ProtectionSessionNodeV1) -> Result<ExactCustodyEnvelopeNodeV1, ()> {
    Ok(ExactCustodyEnvelopeNodeV1::new(
        node.node_public_key().map_err(|_| ())?,
        node.node_custody_public_key().map_err(|_| ())?,
    ))
}

fn load_provider_state(config: Value) -> Result<ConfiguredProtectProvider, ()> {
    let config = serde_json::from_value::<InitConfig>(config).map_err(|_| ())?;
    if !config.base_path.is_empty()
        || !config.allowed_paths.is_empty()
        || config.read_only
        || !config.encryption_key.is_empty()
    {
        return Err(());
    }
    let _ = config.extra;
    Ok(ConfiguredProtectProvider {
        open_replays: BTreeMap::new(),
        sessions: BTreeMap::new(),
        closed_handles: BTreeSet::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvelopeState {
    Absent,
    Present,
}

fn control_request_has_exact_fields(value: &Value, op: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match op {
        "status" | "shutdown" => object.len() == 1 && object.contains_key("op"),
        "init" => object.len() == 2 && object.contains_key("op") && object.contains_key("config"),
        _ => false,
    }
}

fn expected_local_json_runtime_invocation_abi() -> Value {
    json!({
        "schema": "elastos.provider.transfer-abi/v1",
        "transfer": "json",
        "transport": "runtime-local-provider-plane",
        "range_supported": false,
        "progress_supported": false,
        "progress_mode": "none",
        "transport_native_stream": false,
        "backpressure": "not_applicable",
        "cancel_supported": false
    })
}

fn strip_runtime_invocation_envelope(
    value: &mut Value,
    expected_target: &str,
) -> Result<EnvelopeState, ()> {
    let object = value.as_object_mut().ok_or(())?;
    if object.contains_key("_runtime_transfer") {
        return Err(());
    }
    let Some(envelope) = object.remove("_runtime_invocation") else {
        return Ok(EnvelopeState::Absent);
    };
    let envelope = envelope.as_object().ok_or(())?;
    if envelope.len() != 11 {
        return Err(());
    }
    if ![
        "schema",
        "source",
        "target",
        "op",
        "capability",
        "transport",
        "carrier",
        "transfer",
        "range",
        "progress",
        "abi",
    ]
    .into_iter()
    .all(|key| envelope.contains_key(key))
    {
        return Err(());
    }
    let op = object.get("op").and_then(Value::as_str).ok_or(())?;
    let expected_capability = format!("provider:runtime->{expected_target}:{op}");
    let expected_abi = expected_local_json_runtime_invocation_abi();
    if envelope.get("schema").and_then(Value::as_str) != Some("elastos.provider.invocation/v1")
        || envelope.get("source").and_then(Value::as_str) != Some("runtime")
        || envelope.get("target").and_then(Value::as_str) != Some(expected_target)
        || envelope.get("op").and_then(Value::as_str) != Some(op)
        || envelope.get("capability").and_then(Value::as_str) != Some(expected_capability.as_str())
        || envelope.get("transport").and_then(Value::as_str) != Some("runtime-local-provider-plane")
        || envelope.get("carrier") != Some(&Value::Null)
        || envelope.get("transfer").and_then(Value::as_str) != Some("json")
        || envelope.get("range") != Some(&Value::Null)
        || envelope.get("progress") != Some(&Value::Null)
        || envelope.get("abi") != Some(&expected_abi)
    {
        return Err(());
    }
    Ok(EnvelopeState::Present)
}

fn invalid_request() -> ProviderResponse {
    ProviderResponse::error(REQUEST_ERROR_CODE, "protect provider request is invalid")
}

fn typed_ok(response: ProtectProviderResponseV1) -> ProviderResponse {
    match serde_json::to_value(response) {
        Ok(value) => ProviderResponse::ok(value),
        Err(_) => typed_response(ProtectProviderResponseV1::new_failure(
            ProviderFailureCodeV1::InternalFailure,
        )),
    }
}

fn typed_response(
    response: Result<ProtectProviderResponseV1, elastos_protected_content_contracts::ContractError>,
) -> ProviderResponse {
    match response {
        Ok(response) => match serde_json::to_value(response) {
            Ok(value) => ProviderResponse::ok(value),
            Err(_) => {
                ProviderResponse::error(REQUEST_ERROR_CODE, "protect provider request is invalid")
            }
        },
        Err(_) => {
            ProviderResponse::error(REQUEST_ERROR_CODE, "protect provider request is invalid")
        }
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn random_bytes<const N: usize>() -> Result<[u8; N], ()> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).map_err(|_| ())?;
    Ok(bytes)
}

fn random_nonzero_bytes<const N: usize>() -> Result<[u8; N], ()> {
    let bytes = random_bytes::<N>()?;
    if bytes == [0u8; N] {
        return Err(());
    }
    Ok(bytes)
}

fn next_unique_handle<'a>(
    active: impl Iterator<Item = &'a HandleBytes>,
    closed: impl Iterator<Item = &'a HandleBytes>,
) -> Result<HandleBytes, ()> {
    let active: BTreeSet<HandleBytes> = active.copied().collect();
    let closed: BTreeSet<HandleBytes> = closed.copied().collect();
    for _ in 0..128 {
        let candidate = random_nonzero_bytes::<MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1>()?;
        if !active.contains(&candidate) && !closed.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(())
}

fn allocate_sample_ivs(
    prefix: [u8; 4],
    next_counter: &mut u32,
    count: usize,
) -> Result<Vec<[u8; 8]>, ()> {
    let count_u32 = u32::try_from(count).map_err(|_| ())?;
    let end_counter = next_counter.checked_add(count_u32).ok_or(())?;
    let mut counter = *next_counter;
    let mut ivs = Vec::with_capacity(count);
    for _ in 0..count {
        let mut iv = [0u8; 8];
        iv[..4].copy_from_slice(&prefix);
        iv[4..].copy_from_slice(&counter.to_be_bytes());
        ivs.push(iv);
        counter = counter.checked_add(1).ok_or(())?;
    }
    *next_counter = end_counter;
    Ok(ivs)
}

fn trim_btree_map<K: Ord, V>(map: &mut BTreeMap<K, V>, max: usize) {
    while map.len() >= max {
        let _ = map.pop_first();
    }
}

fn trim_btree_set<T: Ord + Clone>(set: &mut BTreeSet<T>, max: usize) {
    while set.len() >= max {
        let Some(first) = set.first().cloned() else {
            break;
        };
        set.remove(&first);
    }
}

pub fn read_provider_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut frame = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if frame.is_empty() && !oversized {
                return Ok(None);
            }
            return Ok(Some(if oversized { Err(()) } else { Ok(frame) }));
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            let chunk = &available[..newline];
            if !oversized {
                if frame.len().saturating_add(chunk.len()) > MAX_PROVIDER_FRAME_BYTES_V1 {
                    oversized = true;
                } else {
                    frame.extend_from_slice(chunk);
                }
            }
            reader.consume(newline + 1);
            return Ok(Some(if oversized { Err(()) } else { Ok(frame) }));
        }
        let consumed = available.len();
        if !oversized {
            if frame.len().saturating_add(consumed) > MAX_PROVIDER_FRAME_BYTES_V1 {
                oversized = true;
            } else {
                frame.extend_from_slice(available);
            }
        }
        reader.consume(consumed);
    }
}

pub fn run_provider_loop<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    provider: &mut ProtectProvider,
) {
    loop {
        let (response, should_shutdown) = match read_provider_frame(input) {
            Ok(Some(Ok(frame))) => provider.handle_frame(&frame),
            Ok(Some(Err(()))) => (invalid_request(), false),
            Ok(None) => break,
            Err(_) => break,
        };
        if serde_json::to_writer(&mut *output, &response).is_err() {
            break;
        }
        if writeln!(output).and_then(|()| output.flush()).is_err() {
            break;
        }
        if should_shutdown {
            break;
        }
    }
}

pub fn run_provider_process() {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();
    let mut provider = ProtectProvider::new();
    run_provider_loop(&mut input, &mut stdout, &mut provider);
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::SigningKey;
    use elastos_protected_content_contracts::{Digest32, NodeCustodyPublicKeyV1, NodePublicKey};
    use elastos_protected_content_provider_contracts::ProtectProviderResponseStatusV1;

    fn init_config() -> Value {
        json!({
            "base_path": "",
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": "",
            "extra": {}
        })
    }

    fn digest(seed: u8) -> Digest32 {
        Digest32::new([seed; 32])
    }

    fn node_public_key(seed: u8) -> NodePublicKey {
        let signing = SigningKey::from_bytes(&[seed; 32]);
        NodePublicKey::new(signing.verifying_key().to_bytes()).unwrap()
    }

    fn node_custody_public_key(seed: u8) -> NodeCustodyPublicKeyV1 {
        let secret = elastos_protected_content_custody::NodeCustodySecretKeyV1::generate().unwrap();
        let mut bytes = *secret.public_key().unwrap().as_bytes();
        bytes[0] ^= seed;
        NodeCustodyPublicKeyV1::new(bytes).unwrap_or_else(|_| secret.public_key().unwrap())
    }

    fn nodes() -> Vec<ProtectionSessionNodeV1> {
        vec![
            ProtectionSessionNodeV1::new(node_public_key(1), node_custody_public_key(1)).unwrap(),
            ProtectionSessionNodeV1::new(node_public_key(2), node_custody_public_key(2)).unwrap(),
            ProtectionSessionNodeV1::new(node_public_key(3), node_custody_public_key(3)).unwrap(),
        ]
    }

    fn make_box(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + content.len());
        out.extend_from_slice(&(u32::try_from(8 + content.len()).unwrap()).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(content);
        out
    }

    fn make_fullbox(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut content = Vec::with_capacity(4 + payload.len());
        content.push(version);
        content.extend_from_slice(&flags.to_be_bytes()[1..]);
        content.extend_from_slice(payload);
        make_box(kind, &content)
    }

    fn make_avc1_entry() -> Vec<u8> {
        let mut payload = vec![0u8; 78];
        payload[24..26].copy_from_slice(&1920u16.to_be_bytes());
        payload[26..28].copy_from_slice(&1080u16.to_be_bytes());
        payload[40..42].copy_from_slice(&1u16.to_be_bytes());
        make_box(b"avc1", &payload)
    }

    fn clear_init_segment() -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
        let stsd = {
            let mut payload = Vec::new();
            payload.extend_from_slice(&1u32.to_be_bytes());
            payload.extend_from_slice(&make_avc1_entry());
            make_fullbox(b"stsd", 0, 0, &payload)
        };
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let hdlr = {
            let mut payload = Vec::new();
            payload.extend_from_slice(&0u32.to_be_bytes());
            payload.extend_from_slice(b"vide");
            payload.extend_from_slice(&[0u8; 12]);
            make_fullbox(b"hdlr", 0, 0, &payload)
        };
        let mdia = {
            let mut content = Vec::new();
            content.extend_from_slice(&hdlr);
            content.extend_from_slice(&minf);
            make_box(b"mdia", &content)
        };
        let trak = {
            let tkhd = {
                let mut payload = vec![0u8; 76];
                payload[8..12].copy_from_slice(&1u32.to_be_bytes());
                make_fullbox(b"tkhd", 0, 0x000007, &payload)
            };
            let mut content = Vec::new();
            content.extend_from_slice(&tkhd);
            content.extend_from_slice(&mdia);
            make_box(b"trak", &content)
        };
        let mvex = {
            let mut payload = Vec::new();
            payload.extend_from_slice(&1u32.to_be_bytes());
            payload.extend_from_slice(&1u32.to_be_bytes());
            payload.extend_from_slice(&0u32.to_be_bytes());
            payload.extend_from_slice(&0u32.to_be_bytes());
            payload.extend_from_slice(&0u32.to_be_bytes());
            let trex = make_fullbox(b"trex", 0, 0, &payload);
            make_box(b"mvex", &trex)
        };
        let moov = {
            let mut content = Vec::new();
            content.extend_from_slice(&trak);
            content.extend_from_slice(&mvex);
            make_box(b"moov", &content)
        };
        [ftyp, moov].concat()
    }

    fn clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
        const TFHD_FLAGS_PRODUCER_V1: u32 = 0x020038;
        const TRUN_FLAG_DATA_OFFSET: u32 = 0x000001;
        const TRUN_FLAG_SAMPLE_SIZE: u32 = 0x000200;

        let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let tfhd = {
            let mut payload_bytes = Vec::new();
            payload_bytes.extend_from_slice(&track_id.to_be_bytes());
            payload_bytes.extend_from_slice(&1u32.to_be_bytes());
            payload_bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_be_bytes());
            payload_bytes.extend_from_slice(&0u32.to_be_bytes());
            make_fullbox(b"tfhd", 0, TFHD_FLAGS_PRODUCER_V1, &payload_bytes)
        };
        let tfdt = make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes());
        let trun = {
            let mut payload_bytes = Vec::new();
            payload_bytes.extend_from_slice(&1u32.to_be_bytes());
            payload_bytes.extend_from_slice(&0i32.to_be_bytes());
            payload_bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_be_bytes());
            make_fullbox(
                b"trun",
                0,
                TRUN_FLAG_DATA_OFFSET | TRUN_FLAG_SAMPLE_SIZE,
                &payload_bytes,
            )
        };
        let traf = {
            let mut content = Vec::new();
            content.extend_from_slice(&tfhd);
            content.extend_from_slice(&tfdt);
            content.extend_from_slice(&trun);
            make_box(b"traf", &content)
        };
        let mut moof = {
            let mut content = Vec::new();
            content.extend_from_slice(&mfhd);
            content.extend_from_slice(&traf);
            make_box(b"moof", &content)
        };
        let data_offset_at = moof.len() - trun.len() + 16;
        let sample_offset = (moof.len() + 8) as i32;
        moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());
        [moof, make_box(b"mdat", payload)].concat()
    }

    fn wrap_request(request: &ProtectProviderRequestV1) -> Vec<u8> {
        let mut value = serde_json::to_value(request).unwrap();
        let op = value["op"].as_str().unwrap().to_string();
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "protect",
                "op": op,
                "capability": format!("provider:runtime->protect:{op}"),
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": null,
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi()
            }),
        );
        serde_json::to_vec(&value).unwrap()
    }

    fn typed_ok_response(response: ProviderResponse) -> ProtectProviderResponseV1 {
        let typed = typed_provider_response(response);
        assert_ne!(typed.status(), ProtectProviderResponseStatusV1::Failure);
        typed
    }

    fn typed_provider_response(response: ProviderResponse) -> ProtectProviderResponseV1 {
        match response {
            ProviderResponse::Ok { data: Some(data) } => {
                ProtectProviderResponseV1::from_json_slice(&serde_json::to_vec(&data).unwrap())
                    .unwrap()
            }
            other => panic!(
                "unexpected response: {}",
                serde_json::to_string(&other).unwrap()
            ),
        }
    }

    fn assert_failure_code(response: ProviderResponse, expected: ProviderFailureCodeV1) {
        let typed = typed_provider_response(response);
        assert_eq!(typed.status(), ProtectProviderResponseStatusV1::Failure);
        assert_eq!(typed.failure_code(), Some(expected));
    }

    fn init_provider() -> ProtectProvider {
        let mut provider = ProtectProvider::new();
        let (init_response, _) = provider.handle_frame(
            &serde_json::to_vec(&json!({"op": "init", "config": init_config()})).unwrap(),
        );
        match init_response {
            ProviderResponse::Ok { .. } => provider,
            other => panic!(
                "unexpected init: {}",
                serde_json::to_string(&other).unwrap()
            ),
        }
    }

    #[test]
    fn status_exposes_exact_runtime_readiness_contract() {
        let mut provider = init_provider();
        let (response, should_shutdown) = provider.handle_frame(br#"{"op":"status"}"#);
        assert!(!should_shutdown);
        let ProviderResponse::Ok { data: Some(data) } = response else {
            panic!("status must return readiness data");
        };
        assert_eq!(
            data,
            json!({
                "provider": "protected-content-protect",
                "version": PROVIDER_VERSION,
                "configured": true,
                "request_schema": PROTECT_PROVIDER_REQUEST_SCHEMA_V1,
                "response_schema": PROTECT_PROVIDER_RESPONSE_SCHEMA_V1,
                "supported_operations": [
                    "status",
                    "open_protection_session",
                    "protect_media_segment",
                    "finalize_protection_session",
                    "cancel_protection_session",
                    "close_protection_session",
                    "shutdown"
                ],
            })
        );
    }

    fn open_request(
        request_id_seed: u8,
        segment_count: u32,
        clear_init: &[u8],
        nodes: Vec<ProtectionSessionNodeV1>,
    ) -> ProtectProviderRequestV1 {
        ProtectProviderRequestV1::new_open_protection_session(
            digest(request_id_seed),
            elastos_protected_content_contracts::ContentAccessIdV1::new([0x41; 16]).unwrap(),
            CustodyPoolIdentityV1::new(digest(0x41), 32).unwrap(),
            CustodyEpochIdentityV1::new(digest(0x42), 32).unwrap(),
            CustodyCommitteeAuthorizationIdentityV1::new(digest(0x43), 32).unwrap(),
            "video/mp4",
            "avc1.640028",
            segment_count,
            clear_init,
            nodes,
        )
        .unwrap()
    }

    #[test]
    fn open_segment_finalize_success_produces_valid_identity_and_bound_envelope() {
        let clear_init = clear_init_segment();
        let clear_segments = [clear_segment(1, b"hello"), clear_segment(1, b"world!")];
        let request = open_request(
            0x31,
            u32::try_from(clear_segments.len()).unwrap(),
            &clear_init,
            nodes(),
        );

        let mut provider = init_provider();

        let opened = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        assert_eq!(
            opened.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionOpened
        );
        let handle = opened.protection_session_handle().unwrap().unwrap();
        let protected_init = opened.protected_init_segment().unwrap().to_vec();

        let mut protected_segments = Vec::new();
        for (index, segment) in clear_segments.iter().enumerate() {
            let response = typed_ok_response(
                provider
                    .handle_frame(&wrap_request(
                        &ProtectProviderRequestV1::new_protect_media_segment(
                            handle,
                            u32::try_from(index).unwrap(),
                            segment,
                        )
                        .unwrap(),
                    ))
                    .0,
            );
            assert_eq!(
                response.status(),
                ProtectProviderResponseStatusV1::MediaSegmentProtected
            );
            protected_segments.push(response.protected_segment().unwrap().to_vec());
        }

        let finalized = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_finalize_protection_session(handle).unwrap(),
                ))
                .0,
        );
        assert_eq!(
            finalized.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionFinalized
        );
        let media_identity = finalized.media_identity().unwrap().unwrap();
        let expected_identity = CencFmp4MediaIdentityV1::new_from_bytes(
            &protected_init,
            &protected_segments,
            "video/mp4",
            "avc1.640028",
        )
        .unwrap();
        assert_eq!(media_identity, expected_identity);
        let validated =
            CencFmp4MediaIdentityV1::validate_structure(&protected_init, &protected_segments)
                .unwrap();
        assert_eq!(validated.protected_track_ids(), &[1]);

        let envelope = finalized.custody_envelope().unwrap().unwrap();
        assert_eq!(
            envelope.manifest().encrypted_content(),
            media_identity.encrypted_content()
        );
        assert_eq!(
            envelope.manifest().custody_pool(),
            request.custody_pool().unwrap().unwrap()
        );
        assert_eq!(
            envelope.manifest().custody_epoch(),
            request.custody_epoch().unwrap().unwrap()
        );
        assert_eq!(
            envelope.manifest().custody_committee_authorization(),
            request.custody_committee_authorization().unwrap().unwrap()
        );
        assert_eq!(envelope.manifest().threshold().required(), 2);
        assert_eq!(envelope.manifest().threshold().total(), 3);
        assert_eq!(envelope.manifest().nodes().len(), 3);
        let mut expected_nodes = request
            .nodes()
            .unwrap()
            .iter()
            .map(|node| node.node_public_key().unwrap())
            .collect::<Vec<_>>();
        expected_nodes.sort_unstable();
        assert_eq!(
            envelope
                .manifest()
                .nodes()
                .iter()
                .map(|node| node.node_public_key())
                .collect::<Vec<_>>(),
            expected_nodes
        );

        let debug = format!("{provider:?}");
        assert!(!debug.contains("key"));
        assert!(!debug.contains("share"));
        assert!(!debug.contains("route"));
        assert!(!debug.contains("path"));
        assert_ne!(protected_init, clear_init);
        assert_ne!(protected_segments[0], clear_segments[0]);
    }

    #[test]
    fn allocate_sample_ivs_is_atomic_on_exhaustion() {
        let mut next_counter = u32::MAX;
        assert!(allocate_sample_ivs([0x11, 0x22, 0x33, 0x44], &mut next_counter, 2).is_err());
        assert_eq!(next_counter, u32::MAX);

        let mut next_counter = u32::MAX - 2;
        let ivs = allocate_sample_ivs([0x11, 0x22, 0x33, 0x44], &mut next_counter, 2).unwrap();
        assert_eq!(next_counter, u32::MAX);
        assert_eq!(ivs.len(), 2);
        assert_eq!(ivs[0], [0x11, 0x22, 0x33, 0x44, 0xff, 0xff, 0xff, 0xfd]);
        assert_eq!(ivs[1], [0x11, 0x22, 0x33, 0x44, 0xff, 0xff, 0xff, 0xfe]);

        let mut next_counter = u32::MAX - 1;
        assert!(allocate_sample_ivs([0x11, 0x22, 0x33, 0x44], &mut next_counter, 2).is_err());
        assert_eq!(next_counter, u32::MAX - 1);
    }

    #[test]
    fn open_replay_is_exact_and_conflicting_reuse_fails() {
        let clear_init = clear_init_segment();
        let nodes = nodes();
        let request = open_request(0x51, 1, &clear_init, nodes.clone());
        let changed = open_request(0x51, 2, &clear_init, nodes);
        let mut provider = init_provider();

        let first = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        let replay = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        assert_eq!(first, replay);

        assert_failure_code(
            provider.handle_frame(&wrap_request(&changed)).0,
            ProviderFailureCodeV1::BindingMismatch,
        );
    }

    #[test]
    fn segment_replay_order_and_finalize_bounds_are_strict() {
        let clear_init = clear_init_segment();
        let segment0 = clear_segment(1, b"hello");
        let segment1 = clear_segment(1, b"world!");
        let mut provider = init_provider();
        let request = open_request(0x61, 2, &clear_init, nodes());
        let opened = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        let handle = opened.protection_session_handle().unwrap().unwrap();

        assert_failure_code(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_protect_media_segment(handle, 1, &segment1)
                        .unwrap(),
                ))
                .0,
            ProviderFailureCodeV1::InvalidRequest,
        );

        let protected0 = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_protect_media_segment(handle, 0, &segment0)
                        .unwrap(),
                ))
                .0,
        );
        let protected0_replay = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_protect_media_segment(handle, 0, &segment0)
                        .unwrap(),
                ))
                .0,
        );
        assert_eq!(protected0, protected0_replay);

        assert_failure_code(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_protect_media_segment(handle, 0, b"HELLO")
                        .unwrap(),
                ))
                .0,
            ProviderFailureCodeV1::BindingMismatch,
        );

        assert_failure_code(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_finalize_protection_session(handle).unwrap(),
                ))
                .0,
            ProviderFailureCodeV1::InvalidRequest,
        );

        let protected1 = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_protect_media_segment(handle, 1, &segment1)
                        .unwrap(),
                ))
                .0,
        );
        let finalized = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_finalize_protection_session(handle).unwrap(),
                ))
                .0,
        );
        let finalized_replay = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_finalize_protection_session(handle).unwrap(),
                ))
                .0,
        );
        assert_eq!(finalized, finalized_replay);

        assert_failure_code(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_protect_media_segment(handle, 2, b"extra")
                        .unwrap(),
                ))
                .0,
            ProviderFailureCodeV1::InvalidRequest,
        );

        let protected_layout = CencFmp4MediaIdentityV1::validate_structure(
            opened.protected_init_segment().unwrap(),
            &[
                protected0.protected_segment().unwrap().to_vec(),
                protected1.protected_segment().unwrap().to_vec(),
            ],
        )
        .unwrap();
        assert_ne!(
            protected_layout.segments()[0].samples()[0].iv(),
            protected_layout.segments()[1].samples()[0].iv()
        );
    }

    #[test]
    fn cancel_close_and_repeated_close_are_typed() {
        let clear_init = clear_init_segment();
        let mut provider = init_provider();
        let request = open_request(0x71, 1, &clear_init, nodes());
        let opened = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        let handle = opened.protection_session_handle().unwrap().unwrap();

        let cancelled = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_cancel_protection_session(handle).unwrap(),
                ))
                .0,
        );
        assert_eq!(
            cancelled.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionCancelled
        );
        let absent = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_close_protection_session(handle).unwrap(),
                ))
                .0,
        );
        assert_eq!(
            absent.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionAlreadyAbsent
        );

        let request = open_request(0x72, 1, &clear_init, nodes());
        let opened = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        let handle = opened.protection_session_handle().unwrap().unwrap();
        let closed = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_close_protection_session(handle).unwrap(),
                ))
                .0,
        );
        assert_eq!(
            closed.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionClosed
        );
        let closed_replay = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_close_protection_session(handle).unwrap(),
                ))
                .0,
        );
        assert_eq!(
            closed_replay.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionAlreadyAbsent
        );
    }

    #[test]
    fn aggregate_cap_and_session_cap_are_enforced_without_permanent_open_exhaustion() {
        let clear_init = clear_init_segment();
        let mut provider = init_provider();

        let mut handles = Vec::new();
        for seed in 1..=MAX_ACTIVE_SESSIONS_V1 {
            let opened = typed_ok_response(
                provider
                    .handle_frame(&wrap_request(&open_request(
                        u8::try_from(seed).unwrap(),
                        1,
                        &clear_init,
                        nodes(),
                    )))
                    .0,
            );
            handles.push(opened.protection_session_handle().unwrap().unwrap());
        }
        assert_failure_code(
            provider
                .handle_frame(&wrap_request(&open_request(0xf1, 1, &clear_init, nodes())))
                .0,
            ProviderFailureCodeV1::BackendUnavailable,
        );
        let _ = typed_ok_response(
            provider
                .handle_frame(&wrap_request(
                    &ProtectProviderRequestV1::new_close_protection_session(handles[0]).unwrap(),
                ))
                .0,
        );
        let reopened = typed_ok_response(
            provider
                .handle_frame(&wrap_request(&open_request(0xf2, 1, &clear_init, nodes())))
                .0,
        );
        assert_eq!(
            reopened.status(),
            ProtectProviderResponseStatusV1::ProtectionSessionOpened
        );

        let mut provider = init_provider();
        let large_payload = vec![0x55; 2 * 1024 * 1024 - 256];
        let large_segment = clear_segment(1, &large_payload);
        let request = open_request(0x81, 16, &clear_init, nodes());
        let opened = typed_ok_response(provider.handle_frame(&wrap_request(&request)).0);
        let handle = opened.protection_session_handle().unwrap().unwrap();
        let mut failed = false;
        for index in 0..16u32 {
            let response = provider.handle_frame(&wrap_request(
                &ProtectProviderRequestV1::new_protect_media_segment(handle, index, &large_segment)
                    .unwrap(),
            ));
            let typed = typed_provider_response(response.0);
            if typed.status() == ProtectProviderResponseStatusV1::Failure {
                assert_eq!(
                    typed.failure_code(),
                    Some(ProviderFailureCodeV1::InvalidRequest)
                );
                failed = true;
                break;
            }
        }
        assert!(
            failed,
            "aggregate cap should reject oversized session retention"
        );
    }
}
