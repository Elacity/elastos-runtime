use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use elastos_protected_content_contracts::{RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1};
use elastos_protected_content_custody::{
    decrypt_validated_cenc_fmp4_segment_to_clear_v1, possession_transcript_v1,
    reconstruct_content_key_into_decrypt_session, rewrite_validated_cenc_fmp4_init_to_clear_v1,
    DecryptSessionReconstructionInputsV1, DecryptSessionSecretKeyV1,
    DecryptSessionWrappedContentKeyV1, RecipientSecretKeyV1,
};
use elastos_protected_content_provider_contracts::{
    DecryptProviderRequestOpV1, DecryptProviderResponseV1, ProviderFailureCodeV1,
    ValidatedCencFmp4MediaSessionLayoutV1, ValidatedDecryptProviderRequestV1,
    DECRYPT_PROVIDER_REQUEST_SCHEMA_V1, DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
    MAX_PROVIDER_FRAME_BYTES_V1, MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1,
};
use rand::{rngs::StdRng, SeedableRng as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

#[cfg(test)]
#[path = "../tests/support.rs"]
mod support;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const INIT_ERROR_CODE: &str = "invalid_config";
const REQUEST_ERROR_CODE: &str = "invalid_request";
const BACKEND_ERROR_CODE: &str = "backend_unavailable";
const MAX_PREPARED_RECIPIENTS_V1: usize = 64;
const MAX_VIEWER_SESSIONS_V1: usize = 64;

type HandleBytes = [u8; MAX_PROVIDER_OPAQUE_HANDLE_BYTES_V1];
type AuditDigest = [u8; 32];

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
    pub fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    pub fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    pub fn error(code: &'static str, message: &'static str) -> Self {
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
    extra: DecryptInitExtraConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecryptInitExtraConfig {
    trusted_runtime_issuer: String,
}

#[derive(Clone)]
struct PrepareReplayEntry {
    request_digest: AuditDigest,
    response: DecryptProviderResponseV1,
    expires_at: u64,
}

#[derive(Clone)]
struct OpenReplayEntry {
    request_digest: AuditDigest,
    response: DecryptProviderResponseV1,
    expires_at: u64,
}

struct PreparedRecipientEntry {
    protected_content_binding: elastos_protected_content_contracts::ProtectedContentBindingV1,
    action: elastos_protected_content_contracts::RightsActionV1,
    recipient_secret: RecipientSecretKeyV1,
    recipient_identity: elastos_protected_content_contracts::RecipientKeyIdentityV1,
    recipient_public_key: elastos_protected_content_contracts::RecipientPublicKeyBytesV1,
    expires_at: u64,
}

struct ViewerSessionEntry {
    audit_request_id: RuntimeReleaseAuditIdV1,
    media_session_layout: ValidatedCencFmp4MediaSessionLayoutV1,
    protected_init_segment: Vec<u8>,
    decrypt_session_secret: DecryptSessionSecretKeyV1,
    wrapped_content_key: DecryptSessionWrappedContentKeyV1,
    wrap_transcript: Vec<u8>,
    expires_at: u64,
}

#[derive(Clone)]
struct HandleTombstone {
    audit_request_id: RuntimeReleaseAuditIdV1,
    request_digest: AuditDigest,
    response: DecryptProviderResponseV1,
    expires_at: u64,
}

struct ConfiguredDecryptProvider {
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    prepared_replays: BTreeMap<AuditDigest, PrepareReplayEntry>,
    prepared_by_handle: BTreeMap<HandleBytes, PreparedRecipientEntry>,
    cancelled_prepared: BTreeMap<HandleBytes, HandleTombstone>,
    open_replays: BTreeMap<AuditDigest, OpenReplayEntry>,
    viewer_by_handle: BTreeMap<HandleBytes, ViewerSessionEntry>,
    closed_viewers: BTreeMap<HandleBytes, HandleTombstone>,
}

pub struct DecryptProvider {
    state: Option<ConfiguredDecryptProvider>,
}

impl std::fmt::Debug for DecryptProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.state {
            Some(state) => formatter
                .debug_struct("DecryptProvider")
                .field("configured", &true)
                .field("prepared_replays", &state.prepared_replays.len())
                .field("prepared_handles", &state.prepared_by_handle.len())
                .field("cancelled_prepared", &state.cancelled_prepared.len())
                .field("open_replays", &state.open_replays.len())
                .field("viewer_handles", &state.viewer_by_handle.len())
                .field("closed_viewers", &state.closed_viewers.len())
                .finish(),
            None => formatter
                .debug_struct("DecryptProvider")
                .field("configured", &false)
                .finish(),
        }
    }
}

impl DecryptProvider {
    pub fn new() -> Self {
        Self { state: None }
    }

    pub fn handle_frame_at(
        &mut self,
        frame: &[u8],
        now_unix_seconds: u64,
    ) -> (ProviderResponse, bool) {
        let mut value = match serde_json::from_slice::<Value>(frame) {
            Ok(value) => value,
            Err(_) => return (invalid_request(), false),
        };
        let envelope = match strip_runtime_invocation_envelope(&mut value, "decrypt") {
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
                "prepare_recipient"
                | "open_viewer_session"
                | "read_viewer_media_part"
                | "cancel_prepared_recipient"
                | "close_viewer_session",
            ) => {
                if !matches!(envelope, EnvelopeState::Present) {
                    return (invalid_request(), false);
                }
                let Ok(bytes) = serde_json::to_vec(&value) else {
                    return (invalid_request(), false);
                };
                (self.handle_decrypt_request(&bytes, now_unix_seconds), false)
            }
            _ => (invalid_request(), false),
        }
    }

    pub fn handle_frame(&mut self, frame: &[u8]) -> (ProviderResponse, bool) {
        self.handle_frame_at(frame, now_unix_seconds())
    }

    #[cfg(test)]
    pub fn handle_line_at(&mut self, line: &str, now_unix_seconds: u64) -> ProviderResponse {
        self.handle_frame_at(line.as_bytes(), now_unix_seconds).0
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
                "decrypt provider configuration is invalid",
            ),
        }
    }

    fn status(&self) -> ProviderResponse {
        ProviderResponse::ok(json!({
            "provider": "protected-content-decrypt",
            "version": PROVIDER_VERSION,
            "configured": self.state.is_some(),
            "supported_operations": [
                "status",
                "prepare_recipient",
                "open_viewer_session",
                "read_viewer_media_part",
                "cancel_prepared_recipient",
                "close_viewer_session",
                "shutdown"
            ],
            "request_schema": DECRYPT_PROVIDER_REQUEST_SCHEMA_V1,
            "response_schema": DECRYPT_PROVIDER_RESPONSE_SCHEMA_V1,
        }))
    }

    fn handle_decrypt_request(&mut self, bytes: &[u8], now_unix_seconds: u64) -> ProviderResponse {
        let Some(state) = self.state.as_mut() else {
            return ProviderResponse::error(
                BACKEND_ERROR_CODE,
                "decrypt provider is not configured",
            );
        };
        state.purge_expired(now_unix_seconds);
        let request = match ValidatedDecryptProviderRequestV1::decode_and_validate_at(
            bytes,
            state.expected_runtime_issuer,
            now_unix_seconds,
        ) {
            Ok(request) => request,
            Err(_) => return invalid_request(),
        };
        let request_digest = digest32(bytes);
        match request.op() {
            DecryptProviderRequestOpV1::PrepareRecipient => {
                let audit_key = audit_digest(request.audit_request_id());
                if let Some(replay) = state.prepared_replays.get(&audit_key) {
                    return if replay.request_digest == request_digest {
                        typed_ok(replay.response.clone())
                    } else {
                        typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::BindingMismatch,
                        ))
                    };
                }
                if state.prepared_by_handle.len() >= MAX_PREPARED_RECIPIENTS_V1
                    || state.prepared_replays.len() >= MAX_PREPARED_RECIPIENTS_V1
                {
                    return typed_response(DecryptProviderResponseV1::new_failure(
                        request.audit_request_id(),
                        ProviderFailureCodeV1::BackendUnavailable,
                    ));
                }
                let recipient_secret = match RecipientSecretKeyV1::generate() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let recipient_public_key = match recipient_secret.public_key() {
                    Ok(value) => {
                        match elastos_protected_content_contracts::RecipientPublicKeyBytesV1::new(
                            *value.as_bytes(),
                        ) {
                            Ok(value) => value,
                            Err(_) => {
                                return typed_response(DecryptProviderResponseV1::new_failure(
                                    request.audit_request_id(),
                                    ProviderFailureCodeV1::InternalFailure,
                                ));
                            }
                        }
                    }
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let recipient_identity = match recipient_secret.identity() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let handle = match next_unique_handle(
                    state.prepared_by_handle.keys(),
                    state.viewer_by_handle.keys(),
                    state.cancelled_prepared.keys(),
                    state.closed_viewers.keys(),
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::BackendUnavailable,
                        ));
                    }
                };
                let response = match DecryptProviderResponseV1::new_prepared_recipient(
                    request.audit_request_id(),
                    handle,
                    recipient_public_key,
                    &recipient_identity,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let entry = PreparedRecipientEntry {
                    protected_content_binding: match request.protected_content_binding() {
                        Ok(value) => value.clone(),
                        Err(_) => return invalid_request(),
                    },
                    action: match request.action() {
                        Ok(value) => value,
                        Err(_) => return invalid_request(),
                    },
                    recipient_secret,
                    recipient_identity,
                    recipient_public_key,
                    expires_at: match request.expires_at() {
                        Ok(value) => value,
                        Err(_) => return invalid_request(),
                    },
                };
                state.prepared_replays.insert(
                    audit_key,
                    PrepareReplayEntry {
                        request_digest,
                        response: response.clone(),
                        expires_at: entry.expires_at,
                    },
                );
                state.prepared_by_handle.insert(handle, entry);
                typed_ok(response)
            }
            DecryptProviderRequestOpV1::OpenViewerSession => {
                let audit_key = audit_digest(request.audit_request_id());
                if let Some(replay) = state.open_replays.get(&audit_key) {
                    return if replay.request_digest == request_digest {
                        typed_ok(replay.response.clone())
                    } else {
                        typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::BindingMismatch,
                        ))
                    };
                }
                if state.viewer_by_handle.len() >= MAX_VIEWER_SESSIONS_V1
                    || state.open_replays.len() >= MAX_VIEWER_SESSIONS_V1
                {
                    return typed_response(DecryptProviderResponseV1::new_failure(
                        request.audit_request_id(),
                        ProviderFailureCodeV1::BackendUnavailable,
                    ));
                }
                let Some(prepared) = state.prepared_by_handle.get(
                    request
                        .prepared_recipient_handle()
                        .expect("validated handle"),
                ) else {
                    return typed_response(DecryptProviderResponseV1::new_failure(
                        request.audit_request_id(),
                        ProviderFailureCodeV1::HandleAbsent,
                    ));
                };
                let operation = match request.authenticated_runtime_release_operation() {
                    Ok(value) => value,
                    Err(_) => return invalid_request(),
                };
                if &prepared.protected_content_binding != operation.binding()
                    || prepared.action != operation.action()
                    || &prepared.recipient_identity != operation.recipient()
                    || prepared.recipient_public_key != operation.statement().recipient_public_key()
                {
                    return typed_response(DecryptProviderResponseV1::new_failure(
                        request.audit_request_id(),
                        ProviderFailureCodeV1::BindingMismatch,
                    ));
                }
                let session_seed = match random_nonzero_bytes() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let reconstruction_rng_seed = match random_nonzero_bytes() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let decrypt_session_secret =
                    match DecryptSessionSecretKeyV1::from_seed(session_seed) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(DecryptProviderResponseV1::new_failure(
                                request.audit_request_id(),
                                ProviderFailureCodeV1::InternalFailure,
                            ));
                        }
                    };
                let mut reconstruction_rng = StdRng::from_seed(reconstruction_rng_seed);
                let decrypt_session_public = match decrypt_session_secret.public_key() {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let wrapped_content_key = match reconstruct_content_key_into_decrypt_session(
                    &DecryptSessionReconstructionInputsV1 {
                        operation,
                        envelope: request.custody_envelope().expect("validated envelope"),
                        contributions: request
                            .signed_node_contributions()
                            .expect("validated contributions"),
                        terminal_receipt: request
                            .signed_terminal_receipt()
                            .expect("validated terminal receipt"),
                        expected_terminal_issuer: request
                            .expected_terminal_issuer()
                            .expect("validated terminal issuer"),
                        recipient_secret: &prepared.recipient_secret,
                        decrypt_session_public: &decrypt_session_public,
                        now: now_unix_seconds,
                    },
                    &mut reconstruction_rng,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::BindingMismatch,
                        ));
                    }
                };
                let mut wrap_transcript = match possession_transcript_v1(
                    operation.binding().profile(),
                    operation.binding().runtime_session_binding(),
                    operation.recipient(),
                    operation.release_request_hash(),
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                wrap_transcript.extend_from_slice(decrypt_session_public.as_bytes());
                let viewer_handle = match next_unique_handle(
                    state.prepared_by_handle.keys(),
                    state.viewer_by_handle.keys(),
                    state.cancelled_prepared.keys(),
                    state.closed_viewers.keys(),
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::BackendUnavailable,
                        ));
                    }
                };
                let response = match DecryptProviderResponseV1::new_viewer_session_opened(
                    request.audit_request_id(),
                    viewer_handle,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                let entry = ViewerSessionEntry {
                    audit_request_id: request.audit_request_id(),
                    media_session_layout: request
                        .media_session_layout()
                        .expect("validated media layout")
                        .clone(),
                    protected_init_segment: request
                        .protected_init_segment()
                        .expect("validated init")
                        .to_vec(),
                    decrypt_session_secret,
                    wrapped_content_key,
                    wrap_transcript,
                    expires_at: operation.statement().expires_at(),
                };
                let prepared_handle = *request
                    .prepared_recipient_handle()
                    .expect("validated handle");
                state.open_replays.insert(
                    audit_key,
                    OpenReplayEntry {
                        request_digest,
                        response: response.clone(),
                        expires_at: entry.expires_at,
                    },
                );
                state.viewer_by_handle.insert(viewer_handle, entry);
                state.prepared_by_handle.remove(&prepared_handle);
                typed_ok(response)
            }
            DecryptProviderRequestOpV1::ReadViewerMediaPart => {
                let handle = *request.viewer_session_handle().expect("validated handle");
                let Some(session) = state.viewer_by_handle.get(&handle) else {
                    return typed_response(DecryptProviderResponseV1::new_failure(
                        request.audit_request_id(),
                        ProviderFailureCodeV1::HandleAbsent,
                    ));
                };
                if session.audit_request_id != request.audit_request_id() {
                    return typed_response(DecryptProviderResponseV1::new_failure(
                        request.audit_request_id(),
                        ProviderFailureCodeV1::BindingMismatch,
                    ));
                }
                let selector = request
                    .viewer_media_part_selector()
                    .expect("validated selector");
                let clear_media_part = if selector.is_init() {
                    match rewrite_validated_cenc_fmp4_init_to_clear_v1(
                        &session.media_session_layout,
                        &session.protected_init_segment,
                    ) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(DecryptProviderResponseV1::new_failure(
                                request.audit_request_id(),
                                ProviderFailureCodeV1::BindingMismatch,
                            ));
                        }
                    }
                } else {
                    let Some(segment_index) = selector.segment_index() else {
                        return invalid_request();
                    };
                    let Some(encrypted_segment) = selector.encrypted_segment() else {
                        return invalid_request();
                    };
                    match decrypt_validated_cenc_fmp4_segment_to_clear_v1(
                        &session.media_session_layout,
                        encrypted_segment,
                        segment_index,
                        &session.decrypt_session_secret,
                        &session.wrapped_content_key,
                        &session.wrap_transcript,
                    ) {
                        Ok(value) => value,
                        Err(_) => {
                            return typed_response(DecryptProviderResponseV1::new_failure(
                                request.audit_request_id(),
                                ProviderFailureCodeV1::BindingMismatch,
                            ));
                        }
                    }
                };
                typed_response(DecryptProviderResponseV1::new_viewer_media_part(
                    request.audit_request_id(),
                    handle,
                    selector.clone(),
                    clear_media_part,
                ))
            }
            DecryptProviderRequestOpV1::CancelPreparedRecipient => {
                let handle = *request
                    .prepared_recipient_handle()
                    .expect("validated handle");
                if let Some(tombstone) = state.cancelled_prepared.get(&handle) {
                    return if tombstone.audit_request_id == request.audit_request_id()
                        && tombstone.request_digest == request_digest
                    {
                        typed_ok(tombstone.response.clone())
                    } else {
                        typed_response(
                            DecryptProviderResponseV1::new_prepared_recipient_already_absent(
                                request.audit_request_id(),
                                handle,
                            ),
                        )
                    };
                }
                let Some(prepared) = state.prepared_by_handle.remove(&handle) else {
                    return typed_response(
                        DecryptProviderResponseV1::new_prepared_recipient_already_absent(
                            request.audit_request_id(),
                            handle,
                        ),
                    );
                };
                let response = match DecryptProviderResponseV1::new_cancelled_prepared_recipient(
                    request.audit_request_id(),
                    handle,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                state.cancelled_prepared.insert(
                    handle,
                    HandleTombstone {
                        audit_request_id: request.audit_request_id(),
                        request_digest,
                        response: response.clone(),
                        expires_at: prepared.expires_at,
                    },
                );
                typed_ok(response)
            }
            DecryptProviderRequestOpV1::CloseViewerSession => {
                let handle = *request.viewer_session_handle().expect("validated handle");
                if let Some(tombstone) = state.closed_viewers.get(&handle) {
                    return if tombstone.audit_request_id == request.audit_request_id()
                        && tombstone.request_digest == request_digest
                    {
                        typed_ok(tombstone.response.clone())
                    } else {
                        typed_response(
                            DecryptProviderResponseV1::new_viewer_session_already_absent(
                                request.audit_request_id(),
                                handle,
                            ),
                        )
                    };
                }
                let Some(session) = state.viewer_by_handle.remove(&handle) else {
                    return typed_response(
                        DecryptProviderResponseV1::new_viewer_session_already_absent(
                            request.audit_request_id(),
                            handle,
                        ),
                    );
                };
                let response = match DecryptProviderResponseV1::new_closed_viewer_session(
                    request.audit_request_id(),
                    handle,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(DecryptProviderResponseV1::new_failure(
                            request.audit_request_id(),
                            ProviderFailureCodeV1::InternalFailure,
                        ));
                    }
                };
                state.closed_viewers.insert(
                    handle,
                    HandleTombstone {
                        audit_request_id: request.audit_request_id(),
                        request_digest,
                        response: response.clone(),
                        expires_at: session.expires_at,
                    },
                );
                typed_ok(response)
            }
        }
    }
}

impl Default for DecryptProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfiguredDecryptProvider {
    fn purge_expired(&mut self, now_unix_seconds: u64) {
        self.prepared_replays
            .retain(|_, entry| entry.expires_at > now_unix_seconds);
        self.prepared_by_handle
            .retain(|_, entry| entry.expires_at > now_unix_seconds);
        self.cancelled_prepared
            .retain(|_, entry| entry.expires_at > now_unix_seconds);
        self.open_replays
            .retain(|_, entry| entry.expires_at > now_unix_seconds);
        self.viewer_by_handle
            .retain(|_, entry| entry.expires_at > now_unix_seconds);
        self.closed_viewers
            .retain(|_, entry| entry.expires_at > now_unix_seconds);
    }
}

fn load_provider_state(config: Value) -> Result<ConfiguredDecryptProvider, ()> {
    let init: InitConfig = serde_json::from_value(config).map_err(|_| ())?;
    if !init.base_path.is_empty()
        || !init.allowed_paths.is_empty()
        || init.read_only
        || !init.encryption_key.is_empty()
    {
        return Err(());
    }
    let expected_runtime_issuer = parse_runtime_issuer_hex(&init.extra.trusted_runtime_issuer)?;
    Ok(ConfiguredDecryptProvider {
        expected_runtime_issuer,
        prepared_replays: BTreeMap::new(),
        prepared_by_handle: BTreeMap::new(),
        cancelled_prepared: BTreeMap::new(),
        open_replays: BTreeMap::new(),
        viewer_by_handle: BTreeMap::new(),
        closed_viewers: BTreeMap::new(),
    })
}

fn parse_runtime_issuer_hex(value: &str) -> Result<RuntimeOperationIssuerKeyV1, ()> {
    let stripped = value.strip_prefix("0x").unwrap_or(value);
    if stripped.len() != 64 {
        return Err(());
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in stripped.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|_| ())?;
        bytes[index] = u8::from_str_radix(text, 16).map_err(|_| ())?;
    }
    RuntimeOperationIssuerKeyV1::new(bytes).map_err(|_| ())
}

fn digest32(bytes: &[u8]) -> AuditDigest {
    Sha256::digest(bytes).into()
}

fn audit_digest(audit_request_id: RuntimeReleaseAuditIdV1) -> AuditDigest {
    *audit_request_id.digest().as_bytes()
}

fn random_nonzero_bytes() -> Result<[u8; 32], ()> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| ())?;
    if bytes == [0u8; 32] {
        return Err(());
    }
    Ok(bytes)
}

fn next_unique_handle<'a>(
    prepared_handles: impl Iterator<Item = &'a HandleBytes>,
    viewer_handles: impl Iterator<Item = &'a HandleBytes>,
    cancelled_handles: impl Iterator<Item = &'a HandleBytes>,
    closed_handles: impl Iterator<Item = &'a HandleBytes>,
) -> Result<HandleBytes, ()> {
    let mut occupied = BTreeMap::new();
    for handle in prepared_handles {
        occupied.insert(*handle, ());
    }
    for handle in viewer_handles {
        occupied.insert(*handle, ());
    }
    for handle in cancelled_handles {
        occupied.insert(*handle, ());
    }
    for handle in closed_handles {
        occupied.insert(*handle, ());
    }
    for _ in 0..32 {
        let candidate = random_nonzero_bytes()?;
        if !occupied.contains_key(&candidate) {
            return Ok(candidate);
        }
    }
    Err(())
}

fn typed_response(
    result: Result<DecryptProviderResponseV1, impl std::fmt::Debug>,
) -> ProviderResponse {
    let Ok(response) = result else {
        return ProviderResponse::error(BACKEND_ERROR_CODE, "decrypt provider response failed");
    };
    let Ok(bytes) = response.to_json_vec() else {
        return ProviderResponse::error(BACKEND_ERROR_CODE, "decrypt provider response failed");
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => ProviderResponse::ok(value),
        Err(_) => ProviderResponse::error(BACKEND_ERROR_CODE, "decrypt provider response failed"),
    }
}

fn typed_ok(response: DecryptProviderResponseV1) -> ProviderResponse {
    let Ok(bytes) = response.to_json_vec() else {
        return ProviderResponse::error(BACKEND_ERROR_CODE, "decrypt provider response failed");
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => ProviderResponse::ok(value),
        Err(_) => ProviderResponse::error(BACKEND_ERROR_CODE, "decrypt provider response failed"),
    }
}

fn invalid_request() -> ProviderResponse {
    ProviderResponse::error(REQUEST_ERROR_CODE, "decrypt provider request is invalid")
}

fn control_request_has_exact_fields(value: &Value, op: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match op {
        "init" => object.len() == 2 && object.contains_key("op") && object.contains_key("config"),
        "status" | "shutdown" => object.len() == 1 && object.contains_key("op"),
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvelopeState {
    Absent,
    Present,
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
    provider: &mut DecryptProvider,
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
    let mut provider = DecryptProvider::new();
    run_provider_loop(&mut input, &mut stdout, &mut provider);
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::SigningKey;
    use elastos_protected_content_contracts::{
        CustodyEnvelopeV1, ProtectedContentBindingV1, RightsActionV1, RuntimeReleaseAuditIdV1,
        SignedNodeContributionV1, SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1,
        TerminalReceiptIssuerKey,
    };
    use elastos_protected_content_provider_contracts::{
        CencFmp4MediaIdentityV1, DecryptProviderRequestV1, DecryptProviderResponseStatusV1,
        ProviderFailureCodeV1, ViewerMediaPartSelectorV1,
    };

    use super::support::{
        binding_for_envelope, custody_envelope_for_media, digest, make_signed_node_contribution,
        make_signed_runtime_release_operation, make_signed_terminal_receipt, media_components,
        runtime_issuer, runtime_signing_key, wallet, MEDIA_CODECS_V1, MEDIA_MIME_TYPE_V1,
    };

    const NOW: u64 = 2_000_000_000;

    fn init_config(runtime_seed: u8) -> Value {
        json!({
            "base_path": "",
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": "",
            "extra": {
                "trusted_runtime_issuer": format!(
                    "0x{}",
                    hex::encode(runtime_signing_key(runtime_seed).verifying_key().to_bytes())
                )
            }
        })
    }

    fn wrap_request(value: Value) -> String {
        let mut value = value;
        let op = value.get("op").and_then(Value::as_str).unwrap().to_string();
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": op,
                "capability": format!("provider:runtime->decrypt:{op}"),
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": null,
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi()
            }),
        );
        serde_json::to_string(&value).unwrap()
    }

    fn request_json(request: &DecryptProviderRequestV1) -> Value {
        serde_json::to_value(request).unwrap()
    }

    fn typed_response(response: ProviderResponse) -> DecryptProviderResponseV1 {
        match response {
            ProviderResponse::Ok { data: Some(data) } => {
                DecryptProviderResponseV1::from_json_slice(&serde_json::to_vec(&data).unwrap())
                    .unwrap()
            }
            other => panic!(
                "expected typed ok response, got {:?}",
                serde_json::to_value(&other).unwrap()
            ),
        }
    }

    fn response_error_code(response: &ProviderResponse) -> Option<&'static str> {
        match response {
            ProviderResponse::Error { code, .. } => Some(*code),
            _ => None,
        }
    }

    fn prepare_request(
        binding: &ProtectedContentBindingV1,
        audit_request_id: RuntimeReleaseAuditIdV1,
        runtime_seed: u8,
    ) -> DecryptProviderRequestV1 {
        DecryptProviderRequestV1::new_prepare_recipient(
            binding,
            audit_request_id,
            RightsActionV1::View,
            runtime_issuer(runtime_seed),
            NOW,
            NOW + 30,
        )
        .unwrap()
    }

    struct OpenRequestInputs<'a> {
        prepared_handle: HandleBytes,
        operation: &'a SignedRuntimeReleaseOperationV1,
        envelope: &'a CustodyEnvelopeV1,
        media_identity: &'a CencFmp4MediaIdentityV1,
        init_segment: &'a [u8],
        contributions: &'a [SignedNodeContributionV1],
        terminal_receipt: &'a SignedTerminalReceiptV1,
        issuer_seed: u8,
    }

    fn open_request(inputs: OpenRequestInputs<'_>) -> DecryptProviderRequestV1 {
        DecryptProviderRequestV1::new_open_viewer_session(
            inputs.prepared_handle,
            inputs.operation,
            TerminalReceiptIssuerKey::new(
                SigningKey::from_bytes(&[inputs.issuer_seed; 32])
                    .verifying_key()
                    .to_bytes(),
            )
            .unwrap(),
            inputs.envelope,
            inputs.media_identity,
            inputs.init_segment,
            inputs.contributions,
            inputs.terminal_receipt,
        )
        .unwrap()
    }

    fn read_request(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_handle: HandleBytes,
        selector: ViewerMediaPartSelectorV1,
    ) -> DecryptProviderRequestV1 {
        DecryptProviderRequestV1::new_read_viewer_media_part(
            audit_request_id,
            viewer_handle,
            selector,
        )
        .unwrap()
    }

    fn cancel_request(
        audit_request_id: RuntimeReleaseAuditIdV1,
        prepared_handle: HandleBytes,
    ) -> DecryptProviderRequestV1 {
        DecryptProviderRequestV1::new_cancel_prepared_recipient(audit_request_id, prepared_handle)
            .unwrap()
    }

    fn close_request(
        audit_request_id: RuntimeReleaseAuditIdV1,
        viewer_handle: HandleBytes,
    ) -> DecryptProviderRequestV1 {
        DecryptProviderRequestV1::new_close_viewer_session(audit_request_id, viewer_handle).unwrap()
    }

    #[test]
    fn prepare_open_read_and_close_flow_is_exact_and_redacted() {
        let runtime_seed = 0x42;
        let media_seed = 0x21;
        let envelope = custody_envelope_for_media(media_seed, NOW);
        let binding = binding_for_envelope(&envelope);
        let (init_segment, encrypted_segments, _, _) = media_components(media_seed);
        let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            MEDIA_MIME_TYPE_V1,
            MEDIA_CODECS_V1,
        )
        .unwrap();
        let prepare_audit = RuntimeReleaseAuditIdV1::new(digest(0xa1)).unwrap();
        let mut provider = DecryptProvider::new();
        assert!(matches!(
            provider.init(init_config(runtime_seed)),
            ProviderResponse::Ok { .. }
        ));

        let prepare = prepare_request(&binding, prepare_audit, runtime_seed);
        let prepared =
            typed_response(provider.handle_line_at(&wrap_request(request_json(&prepare)), NOW + 1));
        assert_eq!(
            prepared.status(),
            DecryptProviderResponseStatusV1::PreparedRecipient
        );

        let open_audit = RuntimeReleaseAuditIdV1::new(digest(0xa2)).unwrap();
        let operation = make_signed_runtime_release_operation(
            runtime_seed,
            open_audit,
            &envelope,
            prepared.recipient_public_key().unwrap(),
            prepared.recipient_identity().unwrap(),
            NOW,
        );
        let contributions = vec![
            make_signed_node_contribution(&operation, &envelope, runtime_seed, 1, NOW),
            make_signed_node_contribution(&operation, &envelope, runtime_seed, 2, NOW),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61, NOW);
        let open = open_request(OpenRequestInputs {
            prepared_handle: *prepared.prepared_recipient_handle().unwrap(),
            operation: &operation,
            envelope: &envelope,
            media_identity: &media_identity,
            init_segment: &init_segment,
            contributions: &contributions,
            terminal_receipt: &terminal,
            issuer_seed: 0x61,
        });
        let opened =
            typed_response(provider.handle_line_at(&wrap_request(request_json(&open)), NOW + 7));
        assert_eq!(
            opened.status(),
            DecryptProviderResponseStatusV1::ViewerSessionOpened
        );

        let read_init = read_request(
            open_audit,
            *opened.viewer_session_handle().unwrap(),
            ViewerMediaPartSelectorV1::init(),
        );
        let clear_init = typed_response(
            provider.handle_line_at(&wrap_request(request_json(&read_init)), NOW + 8),
        );
        assert_eq!(
            clear_init.status(),
            DecryptProviderResponseStatusV1::ViewerMediaPart
        );
        assert!(clear_init
            .clear_media_part()
            .unwrap()
            .windows(4)
            .any(|w| w == b"avc1"));
        assert!(!clear_init
            .clear_media_part()
            .unwrap()
            .windows(4)
            .any(|w| w == b"sinf"));

        let selector =
            ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap();
        let read_segment = read_request(
            open_audit,
            *opened.viewer_session_handle().unwrap(),
            selector.clone(),
        );
        let clear_segment = typed_response(
            provider.handle_line_at(&wrap_request(request_json(&read_segment)), NOW + 8),
        );
        assert_eq!(
            clear_segment.status(),
            DecryptProviderResponseStatusV1::ViewerMediaPart
        );
        assert_eq!(
            clear_segment.viewer_media_part_selector().unwrap(),
            &selector
        );
        assert_ne!(
            clear_segment.clear_media_part().unwrap(),
            encrypted_segments[1].as_slice()
        );
        assert_eq!(encrypted_segments[1], media_components(media_seed).1[1]);

        let closed = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&close_request(
                open_audit,
                *opened.viewer_session_handle().unwrap(),
            ))),
            NOW + 9,
        ));
        assert_eq!(
            closed.status(),
            DecryptProviderResponseStatusV1::ClosedViewerSession
        );

        let debug = format!("{provider:?}");
        assert!(debug.contains("configured"));
        assert!(!debug.contains("carrier"));
        assert!(!debug.contains("segx"));
    }

    #[test]
    fn init_rejects_old_raw_shape_and_unknown_generic_fields() {
        let runtime_seed = 0x42;
        let mut provider = DecryptProvider::new();

        assert_eq!(
            response_error_code(&provider.init(json!({
                "trusted_runtime_issuer": format!(
                    "0x{}",
                    hex::encode(runtime_signing_key(runtime_seed).verifying_key().to_bytes())
                )
            }))),
            Some(INIT_ERROR_CODE)
        );

        assert_eq!(
            response_error_code(&provider.init(json!({
                "base_path": "",
                "allowed_paths": [],
                "read_only": false,
                "encryption_key": "",
                "extra": {
                    "trusted_runtime_issuer": format!(
                        "0x{}",
                        hex::encode(runtime_signing_key(runtime_seed).verifying_key().to_bytes())
                    )
                },
                "ambient": true
            }))),
            Some(INIT_ERROR_CODE)
        );
    }

    #[test]
    fn exact_prepare_and_open_replay_return_same_response_and_conflicts_fail() {
        let runtime_seed = 0x42;
        let media_seed = 0x22;
        let envelope = custody_envelope_for_media(media_seed, NOW);
        let binding = binding_for_envelope(&envelope);
        let (init_segment, encrypted_segments, _, _) = media_components(media_seed);
        let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            MEDIA_MIME_TYPE_V1,
            MEDIA_CODECS_V1,
        )
        .unwrap();
        let prepare_audit = RuntimeReleaseAuditIdV1::new(digest(0xb1)).unwrap();
        let mut provider = DecryptProvider::new();
        let _ = provider.init(init_config(runtime_seed));

        let prepare = prepare_request(&binding, prepare_audit, runtime_seed);
        let prepare_line = wrap_request(request_json(&prepare));
        let prepared_a = typed_response(provider.handle_line_at(&prepare_line, NOW + 1));
        let prepared_b = typed_response(provider.handle_line_at(&prepare_line, NOW + 2));
        assert_eq!(prepared_a, prepared_b);

        let conflict_binding = ProtectedContentBindingV1::new(
            binding.encrypted_content().clone(),
            binding.key_envelope().clone(),
            binding.rights_policy().clone(),
            binding.profile(),
            wallet(8),
            binding.runtime_session_binding(),
        )
        .unwrap();
        let conflict_prepare = prepare_request(&conflict_binding, prepare_audit, runtime_seed);
        let prepare_conflict = typed_response(
            provider.handle_line_at(&wrap_request(request_json(&conflict_prepare)), NOW + 2),
        );
        assert_eq!(
            prepare_conflict.failure_code().unwrap(),
            ProviderFailureCodeV1::BindingMismatch
        );

        let open_audit = RuntimeReleaseAuditIdV1::new(digest(0xb2)).unwrap();
        let operation = make_signed_runtime_release_operation(
            runtime_seed,
            open_audit,
            &envelope,
            prepared_a.recipient_public_key().unwrap(),
            prepared_a.recipient_identity().unwrap(),
            NOW,
        );
        let contributions = vec![
            make_signed_node_contribution(&operation, &envelope, runtime_seed, 1, NOW),
            make_signed_node_contribution(&operation, &envelope, runtime_seed, 2, NOW),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61, NOW);
        let open = open_request(OpenRequestInputs {
            prepared_handle: *prepared_a.prepared_recipient_handle().unwrap(),
            operation: &operation,
            envelope: &envelope,
            media_identity: &media_identity,
            init_segment: &init_segment,
            contributions: &contributions,
            terminal_receipt: &terminal,
            issuer_seed: 0x61,
        });
        let open_line = wrap_request(request_json(&open));
        let opened_a = typed_response(provider.handle_line_at(&open_line, NOW + 7));
        let opened_b = typed_response(provider.handle_line_at(&open_line, NOW + 8));
        assert_eq!(opened_a, opened_b);

        let conflicting_open = open_request(OpenRequestInputs {
            prepared_handle: [0x11; 32],
            operation: &operation,
            envelope: &envelope,
            media_identity: &media_identity,
            init_segment: &init_segment,
            contributions: &contributions,
            terminal_receipt: &terminal,
            issuer_seed: 0x61,
        });
        let open_conflict = typed_response(
            provider.handle_line_at(&wrap_request(request_json(&conflicting_open)), NOW + 8),
        );
        assert_eq!(
            open_conflict.failure_code().unwrap(),
            ProviderFailureCodeV1::BindingMismatch
        );
    }

    #[test]
    fn wrong_issuer_handle_expiry_capacity_and_tamper_fail_closed() {
        let runtime_seed = 0x42;
        let media_seed = 0x23;
        let envelope = custody_envelope_for_media(media_seed, NOW);
        let binding = binding_for_envelope(&envelope);
        let (init_segment, encrypted_segments, _, _) = media_components(media_seed);
        let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            MEDIA_MIME_TYPE_V1,
            MEDIA_CODECS_V1,
        )
        .unwrap();
        let mut provider = DecryptProvider::new();
        let _ = provider.init(init_config(runtime_seed));

        let wrong_issuer_prepare = DecryptProviderRequestV1::new_prepare_recipient(
            &binding,
            RuntimeReleaseAuditIdV1::new(digest(0xc1)).unwrap(),
            RightsActionV1::View,
            runtime_issuer(0x55),
            NOW,
            NOW + 10,
        )
        .unwrap();
        let wrong_issuer =
            provider.handle_line_at(&wrap_request(request_json(&wrong_issuer_prepare)), NOW + 1);
        assert_eq!(response_error_code(&wrong_issuer), Some(REQUEST_ERROR_CODE));

        let prepare_audit = RuntimeReleaseAuditIdV1::new(digest(0xc2)).unwrap();
        let prepared = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&prepare_request(
                &binding,
                prepare_audit,
                runtime_seed,
            ))),
            NOW + 1,
        ));
        let absent_cancel = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&cancel_request(
                RuntimeReleaseAuditIdV1::new(digest(0xc3)).unwrap(),
                [0x11; 32],
            ))),
            NOW + 1,
        ));
        assert_eq!(
            absent_cancel.status(),
            DecryptProviderResponseStatusV1::PreparedRecipientAlreadyAbsent
        );

        let operation = make_signed_runtime_release_operation(
            runtime_seed,
            RuntimeReleaseAuditIdV1::new(digest(0xc4)).unwrap(),
            &envelope,
            prepared.recipient_public_key().unwrap(),
            prepared.recipient_identity().unwrap(),
            NOW,
        );
        let contributions = vec![
            make_signed_node_contribution(&operation, &envelope, runtime_seed, 1, NOW),
            make_signed_node_contribution(&operation, &envelope, runtime_seed, 2, NOW),
        ];
        let terminal = make_signed_terminal_receipt(&operation, &contributions, 0x61, NOW);
        let opened = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&open_request(OpenRequestInputs {
                prepared_handle: *prepared.prepared_recipient_handle().unwrap(),
                operation: &operation,
                envelope: &envelope,
                media_identity: &media_identity,
                init_segment: &init_segment,
                contributions: &contributions,
                terminal_receipt: &terminal,
                issuer_seed: 0x61,
            }))),
            NOW + 7,
        ));
        let wrong_segment = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&read_request(
                RuntimeReleaseAuditIdV1::new(digest(0xc4)).unwrap(),
                *opened.viewer_session_handle().unwrap(),
                ViewerMediaPartSelectorV1::segment(3, encrypted_segments[0].clone()).unwrap(),
            ))),
            NOW + 8,
        ));
        assert_eq!(
            wrong_segment.failure_code().unwrap(),
            ProviderFailureCodeV1::BindingMismatch
        );

        let wrong_audit_read = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&read_request(
                RuntimeReleaseAuditIdV1::new(digest(0xc5)).unwrap(),
                *opened.viewer_session_handle().unwrap(),
                ViewerMediaPartSelectorV1::init(),
            ))),
            NOW + 8,
        ));
        assert_eq!(
            wrong_audit_read.failure_code().unwrap(),
            ProviderFailureCodeV1::BindingMismatch
        );

        let expired_read = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&read_request(
                RuntimeReleaseAuditIdV1::new(digest(0xc4)).unwrap(),
                *opened.viewer_session_handle().unwrap(),
                ViewerMediaPartSelectorV1::init(),
            ))),
            NOW + 100,
        ));
        assert_eq!(
            expired_read.failure_code().unwrap(),
            ProviderFailureCodeV1::HandleAbsent
        );

        let mut provider = DecryptProvider::new();
        let _ = provider.init(init_config(runtime_seed));
        for index in 0..MAX_PREPARED_RECIPIENTS_V1 {
            let audit = RuntimeReleaseAuditIdV1::new(digest((index as u8) + 1)).unwrap();
            let response = typed_response(provider.handle_line_at(
                &wrap_request(request_json(&prepare_request(
                    &binding,
                    audit,
                    runtime_seed,
                ))),
                NOW + 1,
            ));
            assert_eq!(
                response.status(),
                DecryptProviderResponseStatusV1::PreparedRecipient
            );
        }
        let over_capacity = typed_response(provider.handle_line_at(
            &wrap_request(request_json(&prepare_request(
                &binding,
                RuntimeReleaseAuditIdV1::new(digest(0xfe)).unwrap(),
                runtime_seed,
            ))),
            NOW + 1,
        ));
        assert_eq!(
            over_capacity.failure_code().unwrap(),
            ProviderFailureCodeV1::BackendUnavailable
        );

        let mut tampered_json = request_json(&open_request(OpenRequestInputs {
            prepared_handle: *prepared.prepared_recipient_handle().unwrap(),
            operation: &operation,
            envelope: &envelope,
            media_identity: &media_identity,
            init_segment: &init_segment,
            contributions: &contributions,
            terminal_receipt: &terminal,
            issuer_seed: 0x61,
        }));
        let contribution = tampered_json["signed_node_contributions"][0]
            .as_array_mut()
            .unwrap();
        contribution[10] = json!(contribution[10].as_u64().unwrap() ^ 1);
        let tampered = provider.handle_line_at(&wrap_request(tampered_json), NOW + 7);
        assert_eq!(response_error_code(&tampered), Some(REQUEST_ERROR_CODE));
    }

    #[test]
    fn invocation_envelope_requires_exact_eleven_field_local_json_shape() {
        let runtime_seed = 0x42;
        let envelope = custody_envelope_for_media(0x31, NOW);
        let binding = binding_for_envelope(&envelope);
        let request = prepare_request(
            &binding,
            RuntimeReleaseAuditIdV1::new(digest(0xd1)).unwrap(),
            runtime_seed,
        );
        let mut provider = DecryptProvider::new();
        let _ = provider.init(init_config(runtime_seed));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": null
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": null,
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi(),
                "host": "127.0.0.1"
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:open_viewer_session",
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": null,
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi(),
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "bytes",
                "range": null,
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi(),
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": {},
                "transfer": "json",
                "range": null,
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi(),
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": {"start": 0, "end": 1},
                "progress": null,
                "abi": expected_local_json_runtime_invocation_abi(),
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": null,
                "progress": {"request_id": "x", "expected_bytes": 1},
                "abi": expected_local_json_runtime_invocation_abi(),
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));

        let mut value = request_json(&request);
        value.as_object_mut().unwrap().insert(
            "_runtime_invocation".to_string(),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "decrypt",
                "op": "prepare_recipient",
                "capability": "provider:runtime->decrypt:prepare_recipient",
                "transport": "runtime-local-provider-plane",
                "carrier": null,
                "transfer": "json",
                "range": null,
                "progress": null,
                "abi": {
                    "schema": "elastos.provider.transfer-abi/v1",
                    "transfer": "bytes",
                    "transport": "runtime-local-provider-plane",
                    "range_supported": false,
                    "progress_supported": false,
                    "progress_mode": "none",
                    "transport_native_stream": false,
                    "backpressure": "not_applicable",
                    "cancel_supported": false
                },
            }),
        );
        let response = provider.handle_line_at(&serde_json::to_string(&value).unwrap(), NOW + 1);
        assert_eq!(response_error_code(&response), Some(REQUEST_ERROR_CODE));
    }
}
