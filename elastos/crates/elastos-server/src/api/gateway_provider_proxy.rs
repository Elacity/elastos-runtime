use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

const LIBRARY_EVENTS_STREAM_KEEPALIVE_SECS: u64 = 15;
const LIBRARY_TRANSFER_RECEIPT_SCHEMA: &str = "elastos.object.transfer.receipt/v1";
const LIBRARY_TRANSFER_REQUEST_ID_HEADER: &str = "x-elastos-request-id";
const LIBRARY_TRANSFER_RECEIPT_HEADER: &str = "x-elastos-transfer-receipt";
const LIBRARY_DOWNLOAD_STREAM_CHUNK_BYTES: usize = 64 * 1024;
const LIBRARY_UPLOAD_SESSION_SCHEMA: &str = "elastos.object.upload-session/v1";
const LIBRARY_UPLOAD_SESSION_TTL_SECS: u64 = 24 * 60 * 60;
static LIBRARY_UPLOAD_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Deserialize)]
pub(super) struct LibraryUploadQuery {
    uri: String,
    #[serde(default)]
    if_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LibraryUploadStartBody {
    uri: String,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    size_bytes: Option<u64>,
    #[serde(default)]
    if_revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LibraryUploadSession {
    schema: String,
    upload_id: String,
    principal_id: String,
    session_id: String,
    uri: String,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    if_revision: Option<String>,
    #[serde(default)]
    total_bytes: Option<u64>,
    received_bytes: u64,
    chunk_count: u64,
    created_at: u64,
    updated_at: u64,
}

pub(super) async fn gateway_library_upload(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(query): Query<LibraryUploadQuery>,
    body: Bytes,
) -> Response {
    let uri = query.uri.trim();
    if uri.is_empty() {
        return (StatusCode::BAD_REQUEST, "library upload uri is required").into_response();
    }
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                "object",
                anyhow::anyhow!("object provider unavailable"),
            )
        }
    };
    if registry
        .registration_for_uri("elastos://object/*")
        .await
        .is_none()
    {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider unavailable"),
        );
    }

    let request_id = format!("object:upload:{}", now_ts());
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: "object.provider.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "requested",
            reason: "Library requested object provider operation upload",
        },
    ) {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider audit failed: {}", err),
        );
    }

    let mime = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut response = match crate::library::handle_library_upload_bytes_runtime(
        &state.data_dir,
        registry,
        &context.principal_id,
        uri,
        mime,
        query.if_revision.as_deref(),
        &body,
    )
    .await
    {
        Ok(value) => value,
        Err(err) => serde_json::json!({
            "status": "error",
            "code": "library_error",
            "message": err.to_string(),
        }),
    };
    let completed = response.get("status").and_then(|value| value.as_str()) == Some("ok");
    let transfer_receipt = if completed {
        let receipt = library_transfer_receipt(
            "upload",
            &request_id,
            uri,
            body.len(),
            Some(body.len()),
            None,
            "completed",
        );
        if let Some(data) = response
            .get_mut("data")
            .and_then(serde_json::Value::as_object_mut)
        {
            data.insert("request_id".to_string(), serde_json::json!(request_id));
            data.insert("receipt".to_string(), receipt.clone());
        }
        Some(receipt)
    } else {
        None
    };
    if completed {
        crate::library::library_event_notifier().notify_waiters();
    }
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: if completed {
                "object.provider.completed"
            } else {
                "object.provider.failed"
            },
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: if completed { "completed" } else { "failed" },
            reason: if completed {
                "Library completed object provider operation upload"
            } else {
                "Library failed object provider operation upload"
            },
        },
    ) {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider audit failed: {}", err),
        );
    }

    let mut response = Json(response).into_response();
    if let Some(receipt) = transfer_receipt.as_ref() {
        insert_library_transfer_headers(response.headers_mut(), receipt);
    }
    response
}

pub(super) async fn gateway_library_upload_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<LibraryUploadStartBody>,
) -> Response {
    let uri = body.uri.trim();
    if uri.is_empty() {
        return library_upload_json_error(
            StatusCode::BAD_REQUEST,
            "missing_uri",
            "library upload uri is required",
        );
    }
    if let Some(size) = body.size_bytes {
        if size as usize > MAX_GATEWAY_FILE_SIZE {
            return library_upload_json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "object_too_large",
                format!(
                    "Library upload exceeds Runtime object upload limit of {} bytes",
                    MAX_GATEWAY_FILE_SIZE
                ),
            );
        }
    }
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                "object",
                anyhow::anyhow!("object provider unavailable"),
            )
        }
    };
    if registry
        .registration_for_uri("elastos://object/*")
        .await
        .is_none()
    {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider unavailable"),
        );
    }

    let upload_id = new_library_upload_session_id();
    let now = now_ts();
    let _ = cleanup_expired_library_upload_sessions(&state.data_dir, now);
    let session = LibraryUploadSession {
        schema: LIBRARY_UPLOAD_SESSION_SCHEMA.to_string(),
        upload_id: upload_id.clone(),
        principal_id: context.principal_id.clone(),
        session_id: context.session_id.clone(),
        uri: uri.to_string(),
        mime: body
            .mime
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        if_revision: body
            .if_revision
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        total_bytes: body.size_bytes,
        received_bytes: 0,
        chunk_count: 0,
        created_at: now,
        updated_at: now,
    };
    if let Err(err) = create_library_upload_session(&state.data_dir, &session) {
        return library_upload_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_session_error",
            err.to_string(),
        );
    }
    let _ = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: "object.provider.upload_session_started",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &upload_id,
            result: "started",
            reason: "Library started chunked object provider upload session",
        },
    );
    Json(serde_json::json!({
        "status": "ok",
        "data": library_upload_session_status(&session),
    }))
    .into_response()
}

pub(super) async fn gateway_library_upload_chunk(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > LIBRARY_UPLOAD_CHUNK_MAX_BYTES {
        return library_upload_json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk_too_large",
            format!(
                "Library upload chunk exceeds Runtime chunk limit of {} bytes",
                LIBRARY_UPLOAD_CHUNK_MAX_BYTES
            ),
        );
    }
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let offset = match library_upload_offset_from_headers(&headers) {
        Ok(offset) => offset,
        Err(err) => {
            return library_upload_json_error(StatusCode::BAD_REQUEST, "invalid_offset", err)
        }
    };
    let mut session = match read_library_upload_session(&state.data_dir, &upload_id) {
        Ok(session) => session,
        Err(err) => {
            return library_upload_json_error(
                StatusCode::NOT_FOUND,
                "upload_session_not_found",
                err.to_string(),
            )
        }
    };
    if let Err(err) = require_library_upload_session_context(&session, &context) {
        return library_upload_json_error(StatusCode::FORBIDDEN, "upload_session_forbidden", err);
    }
    if offset != session.received_bytes {
        return library_upload_json_error(
            StatusCode::BAD_REQUEST,
            "upload_offset_mismatch",
            format!(
                "expected upload offset {}, got {}",
                session.received_bytes, offset
            ),
        );
    }
    let projected = session.received_bytes.saturating_add(body.len() as u64);
    if projected as usize > MAX_GATEWAY_FILE_SIZE {
        return library_upload_json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "object_too_large",
            format!(
                "Library upload exceeds Runtime object upload limit of {} bytes",
                MAX_GATEWAY_FILE_SIZE
            ),
        );
    }
    if let Some(total) = session.total_bytes {
        if projected > total {
            return library_upload_json_error(
                StatusCode::BAD_REQUEST,
                "upload_exceeds_declared_size",
                "upload chunk exceeds declared total size",
            );
        }
    }
    if let Err(err) = append_library_upload_chunk(&state.data_dir, &mut session, &body) {
        return library_upload_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_session_error",
            err.to_string(),
        );
    }
    Json(serde_json::json!({
        "status": "ok",
        "data": library_upload_session_status(&session),
    }))
    .into_response()
}

pub(super) async fn gateway_library_upload_finish(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                "object",
                anyhow::anyhow!("object provider unavailable"),
            )
        }
    };
    if registry
        .registration_for_uri("elastos://object/*")
        .await
        .is_none()
    {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider unavailable"),
        );
    }
    let session = match read_library_upload_session(&state.data_dir, &upload_id) {
        Ok(session) => session,
        Err(err) => {
            return library_upload_json_error(
                StatusCode::NOT_FOUND,
                "upload_session_not_found",
                err.to_string(),
            )
        }
    };
    if let Err(err) = require_library_upload_session_context(&session, &context) {
        return library_upload_json_error(StatusCode::FORBIDDEN, "upload_session_forbidden", err);
    }
    if let Some(total) = session.total_bytes {
        if session.received_bytes != total {
            return library_upload_json_error(
                StatusCode::BAD_REQUEST,
                "upload_incomplete",
                format!(
                    "upload received {} bytes but expected {}",
                    session.received_bytes, total
                ),
            );
        }
    }
    let bytes = match read_library_upload_session_bytes(&state.data_dir, &upload_id) {
        Ok(bytes) => bytes,
        Err(err) => {
            return library_upload_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upload_session_error",
                err.to_string(),
            )
        }
    };
    if bytes.len() as u64 != session.received_bytes {
        return library_upload_json_error(
            StatusCode::BAD_REQUEST,
            "upload_incomplete",
            "upload session byte count does not match received count",
        );
    }

    let request_id = format!("object:upload:{}", now_ts());
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: "object.provider.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "requested",
            reason: "Library requested chunked object provider operation upload",
        },
    ) {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider audit failed: {}", err),
        );
    }

    let mut response = match crate::library::handle_library_upload_bytes_runtime(
        &state.data_dir,
        registry,
        &context.principal_id,
        &session.uri,
        session.mime.as_deref(),
        session.if_revision.as_deref(),
        &bytes,
    )
    .await
    {
        Ok(value) => value,
        Err(err) => serde_json::json!({
            "status": "error",
            "code": "library_error",
            "message": err.to_string(),
        }),
    };
    let completed = response.get("status").and_then(|value| value.as_str()) == Some("ok");
    let transfer_receipt = if completed {
        let receipt = library_chunked_upload_transfer_receipt(&request_id, &session);
        if let Some(data) = response
            .get_mut("data")
            .and_then(serde_json::Value::as_object_mut)
        {
            data.insert("request_id".to_string(), serde_json::json!(request_id));
            data.insert("receipt".to_string(), receipt.clone());
            data.insert(
                "upload_session".to_string(),
                library_upload_session_status(&session),
            );
            data.insert(
                "browser_transport".to_string(),
                serde_json::json!("http-chunk-session"),
            );
        }
        Some(receipt)
    } else {
        None
    };
    if completed {
        let _ = remove_library_upload_session(&state.data_dir, &upload_id);
        crate::library::library_event_notifier().notify_waiters();
    }
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: if completed {
                "object.provider.completed"
            } else {
                "object.provider.failed"
            },
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: if completed { "completed" } else { "failed" },
            reason: if completed {
                "Library completed chunked object provider operation upload"
            } else {
                "Library failed chunked object provider operation upload"
            },
        },
    ) {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider audit failed: {}", err),
        );
    }

    let mut response = Json(response).into_response();
    if let Some(receipt) = transfer_receipt.as_ref() {
        insert_library_transfer_headers(response.headers_mut(), receipt);
    }
    response
}

pub(super) async fn gateway_library_upload_cancel(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let session = match read_library_upload_session(&state.data_dir, &upload_id) {
        Ok(session) => session,
        Err(err) => {
            return library_upload_json_error(
                StatusCode::NOT_FOUND,
                "upload_session_not_found",
                err.to_string(),
            )
        }
    };
    if let Err(err) = require_library_upload_session_context(&session, &context) {
        return library_upload_json_error(StatusCode::FORBIDDEN, "upload_session_forbidden", err);
    }
    if let Err(err) = remove_library_upload_session(&state.data_dir, &upload_id) {
        return library_upload_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upload_session_error",
            err.to_string(),
        );
    }
    Json(serde_json::json!({
        "status": "ok",
        "data": {
            "schema": LIBRARY_UPLOAD_SESSION_SCHEMA,
            "upload_id": upload_id,
            "status": "cancelled",
        },
    }))
    .into_response()
}

pub(super) async fn gateway_library_download(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let mut uris = Vec::new();
    let mut archive_format_value = None;
    for (key, value) in form_urlencoded::parse(raw_query.as_deref().unwrap_or_default().as_bytes())
    {
        match key.as_ref() {
            "uri" => {
                let uri = value.trim().to_string();
                if !uri.is_empty() {
                    uris.push(uri);
                }
            }
            "archive" => archive_format_value = Some(value.into_owned()),
            _ => {}
        }
    }
    if uris.is_empty() {
        return (StatusCode::BAD_REQUEST, "library download uri is required").into_response();
    }
    let archive_format =
        match crate::library::LibraryArchiveFormat::parse(archive_format_value.as_deref()) {
            Ok(format) => format,
            Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
        };
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                "object",
                anyhow::anyhow!("object provider unavailable"),
            )
        }
    };
    if registry
        .registration_for_uri("elastos://object/*")
        .await
        .is_none()
    {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider unavailable"),
        );
    }

    let request_id = format!("object:download:{}", now_ts());
    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: "object.provider.requested",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "requested",
            reason: "Library requested object provider operation download",
        },
    ) {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider audit failed: {}", err),
        );
    }

    let receipt_uri = if uris.len() == 1 {
        uris[0].clone()
    } else {
        format!("selection:{}", uris.len())
    };
    let download_result = if uris.len() == 1 {
        crate::library::handle_library_download_bytes_runtime(
            &state.data_dir,
            registry,
            &context.principal_id,
            &uris[0],
            archive_format,
        )
        .await
    } else {
        crate::library::handle_library_download_selection_bytes_runtime(
            &state.data_dir,
            &context.principal_id,
            &uris,
            archive_format,
        )
        .await
    };
    let download = match download_result {
        Ok(download) => download,
        Err(err) => {
            let _ = append_provider_effect_audit(
                &state.data_dir,
                ProviderEffectAuditInput {
                    capsule_id: LIBRARY_CAPSULE_ID,
                    event_type: "object.provider.failed",
                    principal_id: &context.principal_id,
                    session_id: &context.session_id,
                    request_id: &request_id,
                    result: "failed",
                    reason: "Library failed object provider operation download",
                },
            );
            return gateway_provider_error_response("object", err);
        }
    };

    let (response, range_ok) =
        library_download_response(download, headers.get(RANGE), &request_id, &receipt_uri);
    if !range_ok {
        let _ = append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: LIBRARY_CAPSULE_ID,
                event_type: "object.provider.failed",
                principal_id: &context.principal_id,
                session_id: &context.session_id,
                request_id: &request_id,
                result: "failed",
                reason: "Library failed object provider operation download",
            },
        );
        return response;
    }

    if let Err(err) = append_provider_effect_audit(
        &state.data_dir,
        ProviderEffectAuditInput {
            capsule_id: LIBRARY_CAPSULE_ID,
            event_type: "object.provider.completed",
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id: &request_id,
            result: "completed",
            reason: "Library completed object provider operation download",
        },
    ) {
        return gateway_provider_error_response(
            "object",
            anyhow::anyhow!("object provider audit failed: {}", err),
        );
    }

    response
}

fn library_download_response(
    download: crate::library::LibraryDownloadBytes,
    range_header: Option<&HeaderValue>,
    request_id: &str,
    uri: &str,
) -> (Response, bool) {
    let byte_range = match range_header {
        Some(value) => match library_download_byte_range(value, download.bytes.len()) {
            Ok(range) => Some(range),
            Err(()) => {
                return (
                    library_download_range_not_satisfiable(
                        download.bytes.len(),
                        request_id,
                        uri,
                        value.to_str().unwrap_or_default(),
                    ),
                    false,
                )
            }
        },
        None => None,
    };
    let total_bytes = download.bytes.len();
    let mut response = if let Some((start, end)) = byte_range {
        let bytes = download.bytes[start..=end].to_vec();
        let mut response = Response::new(library_download_stream_body(bytes));
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total_bytes}"))
                .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
        );
        response.headers_mut().insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&(end - start + 1).to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("0")),
        );
        response
    } else {
        let length = total_bytes;
        let mut response = Response::new(library_download_stream_body(download.bytes));
        response.headers_mut().insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&length.to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("0")),
        );
        response
    };
    let headers = response.headers_mut();
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&download.mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&library_download_content_disposition(&download.filename))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"download\"")),
    );
    let served_bytes = byte_range
        .map(|(start, end)| end - start + 1)
        .unwrap_or(total_bytes);
    let receipt = library_transfer_receipt(
        "download",
        request_id,
        uri,
        served_bytes,
        Some(total_bytes),
        byte_range,
        "completed",
    );
    insert_library_transfer_headers(headers, &receipt);
    (response, true)
}

fn library_download_stream_body(bytes: Vec<u8>) -> axum::body::Body {
    let bytes = Bytes::from(bytes);
    let stream = futures_lite::stream::unfold((bytes, 0usize), |(bytes, offset)| async move {
        if offset >= bytes.len() {
            return None;
        }
        let end = (offset + LIBRARY_DOWNLOAD_STREAM_CHUNK_BYTES).min(bytes.len());
        let chunk = bytes.slice(offset..end);
        Some((Ok::<Bytes, Infallible>(chunk), (bytes, end)))
    });
    axum::body::Body::from_stream(stream)
}

fn library_download_range_not_satisfiable(
    total_len: usize,
    request_id: &str,
    uri: &str,
    requested_range: &str,
) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes */{total_len}"))
            .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
    );
    let receipt = serde_json::json!({
        "schema": LIBRARY_TRANSFER_RECEIPT_SCHEMA,
        "op": "download",
        "request_id": request_id,
        "uri": uri,
        "transport": "raw-body",
        "status": "failed",
        "bytes": 0,
        "total_bytes": total_len,
        "requested_range": requested_range,
        "error": "range_not_satisfiable",
    });
    insert_library_transfer_headers(response.headers_mut(), &receipt);
    response
}

fn library_transfer_receipt(
    op: &str,
    request_id: &str,
    uri: &str,
    bytes: usize,
    total_bytes: Option<usize>,
    range: Option<(usize, usize)>,
    status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema": LIBRARY_TRANSFER_RECEIPT_SCHEMA,
        "op": op,
        "request_id": request_id,
        "uri": uri,
        "transport": if op == "download" {
            "http-body-stream"
        } else {
            "raw-body"
        },
        "stream": if op == "download" {
            serde_json::json!({
                "schema": "elastos.object.download-stream/v1",
                "mode": "response_body_chunks",
                "chunk_size": LIBRARY_DOWNLOAD_STREAM_CHUNK_BYTES,
                "backpressure": "http_body_poll",
                "cancel": "drop_body",
                "progress_mode": "transfer_receipt",
            })
        } else {
            serde_json::Value::Null
        },
        "status": status,
        "bytes": bytes,
        "total_bytes": total_bytes,
        "range": range.map(|(start, end)| serde_json::json!({
            "start": start,
            "end": end,
        })),
    })
}

fn insert_library_transfer_headers(headers: &mut HeaderMap, receipt: &serde_json::Value) {
    if let Some(request_id) = receipt
        .get("request_id")
        .and_then(serde_json::Value::as_str)
    {
        if let Ok(value) = HeaderValue::from_str(request_id) {
            headers.insert(LIBRARY_TRANSFER_REQUEST_ID_HEADER, value);
        }
    }
    if let Ok(value) = HeaderValue::from_str(&receipt.to_string()) {
        headers.insert(LIBRARY_TRANSFER_RECEIPT_HEADER, value);
    }
}

fn new_library_upload_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = LIBRARY_UPLOAD_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("object-upload-{nanos}-{counter}")
}

fn library_upload_sessions_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("Runtime").join("ObjectUploadSessions")
}

fn library_upload_session_dir(data_dir: &FsPath, upload_id: &str) -> anyhow::Result<PathBuf> {
    if !library_upload_safe_id(upload_id) {
        anyhow::bail!("invalid upload session id");
    }
    Ok(library_upload_sessions_dir(data_dir).join(upload_id))
}

fn library_upload_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn library_upload_session_metadata_path(
    data_dir: &FsPath,
    upload_id: &str,
) -> anyhow::Result<PathBuf> {
    Ok(library_upload_session_dir(data_dir, upload_id)?.join("session.json"))
}

fn library_upload_session_data_path(data_dir: &FsPath, upload_id: &str) -> anyhow::Result<PathBuf> {
    Ok(library_upload_session_dir(data_dir, upload_id)?.join("payload.bin"))
}

fn create_library_upload_session(
    data_dir: &FsPath,
    session: &LibraryUploadSession,
) -> anyhow::Result<()> {
    let dir = library_upload_session_dir(data_dir, &session.upload_id)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::File::create(dir.join("payload.bin"))?;
    write_library_upload_session(data_dir, session)
}

fn read_library_upload_session(
    data_dir: &FsPath,
    upload_id: &str,
) -> anyhow::Result<LibraryUploadSession> {
    let path = library_upload_session_metadata_path(data_dir, upload_id)?;
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|err| anyhow::anyhow!("invalid upload session: {err}"))
}

fn write_library_upload_session(
    data_dir: &FsPath,
    session: &LibraryUploadSession,
) -> anyhow::Result<()> {
    let path = library_upload_session_metadata_path(data_dir, &session.upload_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(session)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn append_library_upload_chunk(
    data_dir: &FsPath,
    session: &mut LibraryUploadSession,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let path = library_upload_session_data_path(data_dir, &session.upload_id)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&path)?;
    file.seek(SeekFrom::Start(session.received_bytes))?;
    file.write_all(bytes)?;
    session.received_bytes = session.received_bytes.saturating_add(bytes.len() as u64);
    session.chunk_count = session.chunk_count.saturating_add(1);
    session.updated_at = now_ts();
    write_library_upload_session(data_dir, session)
}

fn read_library_upload_session_bytes(
    data_dir: &FsPath,
    upload_id: &str,
) -> anyhow::Result<Vec<u8>> {
    let path = library_upload_session_data_path(data_dir, upload_id)?;
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn remove_library_upload_session(data_dir: &FsPath, upload_id: &str) -> anyhow::Result<()> {
    let dir = library_upload_session_dir(data_dir, upload_id)?;
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    Ok(())
}

fn cleanup_expired_library_upload_sessions(data_dir: &FsPath, now: u64) -> anyhow::Result<()> {
    let dir = library_upload_sessions_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(upload_id) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !library_upload_safe_id(upload_id) {
            continue;
        }
        let Ok(session) = read_library_upload_session(data_dir, upload_id) else {
            continue;
        };
        if now.saturating_sub(session.updated_at) > LIBRARY_UPLOAD_SESSION_TTL_SECS {
            let _ = std::fs::remove_dir_all(path);
        }
    }
    Ok(())
}

fn require_library_upload_session_context(
    session: &LibraryUploadSession,
    context: &HomeLaunchTokenContext,
) -> Result<(), String> {
    if session.principal_id != context.principal_id || session.session_id != context.session_id {
        return Err("upload session belongs to a different Runtime launch context".to_string());
    }
    Ok(())
}

fn library_upload_offset_from_headers(headers: &HeaderMap) -> Result<u64, String> {
    headers
        .get("x-elastos-upload-offset")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "missing x-elastos-upload-offset header".to_string())?
        .parse::<u64>()
        .map_err(|_| "invalid x-elastos-upload-offset header".to_string())
}

fn library_upload_session_status(session: &LibraryUploadSession) -> serde_json::Value {
    serde_json::json!({
        "schema": LIBRARY_UPLOAD_SESSION_SCHEMA,
        "upload_id": session.upload_id,
        "uri": session.uri,
        "received_bytes": session.received_bytes,
        "total_bytes": session.total_bytes,
        "chunk_count": session.chunk_count,
        "chunk_size": LIBRARY_UPLOAD_CHUNK_MAX_BYTES,
        "transport": "http-chunk-session",
        "backpressure": "client_waits_for_chunk_ack",
        "cancel": "DELETE /api/provider/object/upload/:upload_id",
    })
}

fn library_chunked_upload_transfer_receipt(
    request_id: &str,
    session: &LibraryUploadSession,
) -> serde_json::Value {
    let mut receipt = library_transfer_receipt(
        "upload",
        request_id,
        &session.uri,
        session.received_bytes as usize,
        session.total_bytes.map(|value| value as usize),
        None,
        "completed",
    );
    if let Some(receipt) = receipt.as_object_mut() {
        receipt.insert(
            "transport".to_string(),
            serde_json::json!("http-chunk-session"),
        );
        receipt.insert("stream".to_string(), library_upload_session_status(session));
    }
    receipt
}

fn library_upload_json_error(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> Response {
    let mut response = Json(serde_json::json!({
        "status": "error",
        "code": code,
        "message": message.into(),
    }))
    .into_response();
    *response.status_mut() = status;
    response
}

fn library_download_byte_range(
    header: &HeaderValue,
    total_len: usize,
) -> Result<(usize, usize), ()> {
    let value = header.to_str().map_err(|_| ())?.trim();
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Err(());
    };
    if spec.contains(',') {
        return Err(());
    }
    let (start, end) = spec.split_once('-').ok_or(())?;
    if total_len == 0 {
        return Err(());
    }
    if start.is_empty() {
        let suffix_len = end.parse::<usize>().map_err(|_| ())?;
        if suffix_len == 0 {
            return Err(());
        }
        let start = total_len.saturating_sub(suffix_len);
        return Ok((start, total_len - 1));
    }
    let start = start.parse::<usize>().map_err(|_| ())?;
    if start >= total_len {
        return Err(());
    }
    let end = if end.is_empty() {
        total_len - 1
    } else {
        end.parse::<usize>().map_err(|_| ())?.min(total_len - 1)
    };
    if start > end {
        return Err(());
    }
    Ok((start, end))
}

fn library_download_content_disposition(filename: &str) -> String {
    let clean = filename
        .chars()
        .map(|ch| {
            if ch.is_ascii() && !ch.is_ascii_control() && ch != '"' && ch != '\\' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let clean = clean.trim();
    if clean.is_empty() {
        "attachment; filename=\"download\"".to_string()
    } else {
        format!("attachment; filename=\"{clean}\"")
    }
}

pub(super) async fn gateway_library_events_stream(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[LIBRARY_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return gateway_provider_error_response("object", err),
    };
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                "object",
                anyhow::anyhow!("object provider unavailable"),
            )
        }
    };

    let stream_state = LibraryEventsStreamState {
        registry,
        principal_id: context.principal_id,
        cursor: String::new(),
        initialized: false,
    };
    let stream = futures_lite::stream::unfold(stream_state, |mut stream_state| async move {
        loop {
            match library_events_since_cursor(
                &stream_state.registry,
                &stream_state.principal_id,
                &stream_state.cursor,
            )
            .await
            {
                Ok((_events, cursor)) if !stream_state.initialized => {
                    stream_state.cursor = cursor;
                    stream_state.initialized = true;
                }
                Ok((events, cursor)) if !events.is_empty() => {
                    stream_state.cursor = cursor.clone();
                    let event = library_events_sse_event(serde_json::json!({
                        "schema": "elastos.library.events/v1",
                        "cursor": cursor,
                        "events": events,
                    }));
                    return Some((Ok::<SseEvent, Infallible>(event), stream_state));
                }
                Ok(_) => {}
                Err(err) => {
                    let event = library_events_sse_event(serde_json::json!({
                        "schema": "elastos.library.events/v1",
                        "status": "error",
                        "message": err.to_string(),
                        "events": [],
                    }));
                    return Some((Ok::<SseEvent, Infallible>(event), stream_state));
                }
            }
            crate::library::library_event_notifier().notified().await;
        }
    });

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(LIBRARY_EVENTS_STREAM_KEEPALIVE_SECS))
                .text("keepalive"),
        )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    response
}

pub(super) async fn gateway_provider_proxy(
    State(state): State<GatewayState>,
    Path((scheme, op)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let allowed_apps: &[&str] = match scheme.as_str() {
        "documents" => match op.as_str() {
            "summary" | "get" => &[DOCUMENTS_CAPSULE_ID, LIBRARY_CAPSULE_ID],
            _ => &[DOCUMENTS_CAPSULE_ID],
        },
        "object" => match op.as_str() {
            "roots"
            | "list"
            | "stat"
            | "read"
            | "download"
            | "write"
            | "mkdir"
            | "rename"
            | "move"
            | "copy"
            | "trash"
            | "restore"
            | "delete_permanently"
            | "empty_trash"
            | "status"
            | "sync"
            | "extract_archive"
            | "archive_entries"
            | "archive_preview_entry"
            | "archive_extract_entries"
            | "compress_archive"
            | "publish"
            | "unpublish"
            | "repair"
            | "share"
            | "shared_access"
            | "events" => &[LIBRARY_CAPSULE_ID],
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    "Gateway provider operation not found",
                )
                    .into_response()
            }
        },
        "chain" => match op.as_str() {
            "networks" | "status" | "block_number" | "sync_health" | "node_lifecycle" => {
                &[SYSTEM_CAPSULE_ID]
            }
            "balance" => &[SYSTEM_CAPSULE_ID, WALLET_CAPSULE_ID],
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    "Gateway provider operation not found",
                )
                    .into_response()
            }
        },
        "net" => match op.as_str() {
            "status" | "resolve" | "connect" | "stream" | "http" => &[BROWSER_CAPSULE_ID],
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    "Gateway provider operation not found",
                )
                    .into_response()
            }
        },
        "inspect" => match op.as_str() {
            "capsules" | "capsule" | "self" | "plan" | "request_act" => &[SYSTEM_CAPSULE_ID],
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    "Gateway provider operation not found",
                )
                    .into_response()
            }
        },
        _ => return (StatusCode::NOT_FOUND, "Gateway provider not found").into_response(),
    };
    let context =
        match require_home_launch_token_for_any_context(&state.data_dir, &headers, allowed_apps) {
            Ok(context) => context,
            Err(err) => return gateway_provider_error_response(&scheme, err),
        };
    let principal_id = context.principal_id.clone();
    let session_id = context.session_id.clone();
    let registry = match state.provider_registry.as_ref().cloned() {
        Some(registry) => registry,
        None => {
            return gateway_provider_error_response(
                &scheme,
                anyhow::anyhow!("{} provider unavailable", scheme),
            )
        }
    };
    let mut request = if body.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "provider request body must be a JSON object",
                )
                    .into_response();
            }
            Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
        }
    };
    if let Some(field) = provider_proxy_runtime_metadata_field(&request) {
        return (
            StatusCode::BAD_REQUEST,
            format!("provider request must not predeclare Runtime metadata field {field}"),
        )
            .into_response();
    }
    request["op"] = serde_json::Value::String(op.clone());
    if scheme == "documents" || scheme == "object" || scheme == "net" {
        request["principal_id"] = serde_json::Value::String(principal_id.clone());
    }
    if scheme == "object" && op == "shared_access" {
        if let Some(object) = request.as_object_mut() {
            object.remove("recipient_proof");
            if object
                .get("recipient")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                == Some(principal_id.as_str())
                && context.proof_binding_id.is_some()
            {
                object.insert(
                    "recipient_proof".to_string(),
                    serde_json::json!({
                        "schema": "elastos.library.recipient-proof/v1",
                        "source": "runtime-launch-grant",
                        "recipient": principal_id,
                        "principal_id": principal_id,
                        "proof_binding_id": context.proof_binding_id.as_deref().unwrap_or_default(),
                        "session_id": session_id,
                    }),
                );
            }
        }
    }

    if scheme == "net" && op == "http" {
        return gateway_browser::gateway_browser_net_http(registry.as_ref(), &request).await;
    }
    if scheme == "net" && op == "stream" {
        return (
            StatusCode::GONE,
            "legacy /api/provider/net/stream is disabled; Browser streams must be opened through /api/apps/browser/open",
        )
            .into_response();
    }
    if scheme == "inspect" && op == "request_act" {
        return gateway_inspect_action_request(&state, &context, &request).await;
    }

    let chain_lifecycle_audit = chain_lifecycle_effect_audit(&scheme, &op, &request);
    let library_audit = (scheme == "object").then(|| format!("object:{op}:{}", now_ts()));
    if let Some(audit) = &chain_lifecycle_audit {
        if let Err(err) = append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: SYSTEM_CAPSULE_ID,
                event_type: "chain.node_lifecycle.requested",
                principal_id: &principal_id,
                session_id: &session_id,
                request_id: &audit.request_id,
                result: "requested",
                reason: &format!(
                    "System requested chain node lifecycle action {} for {}",
                    audit.action, audit.network
                ),
            },
        ) {
            return gateway_provider_error_response(
                &scheme,
                anyhow::anyhow!("chain node lifecycle audit failed: {}", err),
            );
        }
    }
    if let Some(request_id) = &library_audit {
        if let Err(err) = append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: LIBRARY_CAPSULE_ID,
                event_type: "object.provider.requested",
                principal_id: &principal_id,
                session_id: &session_id,
                request_id,
                result: "requested",
                reason: &format!("Library requested object provider operation {op}"),
            },
        ) {
            return gateway_provider_error_response(
                &scheme,
                anyhow::anyhow!("object provider audit failed: {}", err),
            );
        }
    }

    let response = if scheme == "object"
        && (library_operation_needs_runtime_coordinator(&op)
            || library_request_targets_webspace(&request))
    {
        crate::library::handle_object_provider_runtime_request(
            &state.data_dir,
            Arc::clone(&registry),
            &request,
        )
        .await
    } else {
        match registry.send_raw(&scheme, &request).await {
            Ok(value)
                if scheme == "net"
                    && value.get("status").and_then(|v| v.as_str()) == Some("error") =>
            {
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("net provider unavailable");
                return gateway_provider_error_response(
                    &scheme,
                    anyhow::anyhow!("net provider unavailable: {}", message),
                );
            }
            Ok(value) => value,
            Err(err) if scheme == "net" => {
                return gateway_provider_error_response(
                    &scheme,
                    anyhow::anyhow!("net provider unavailable: {}", err),
                )
            }
            Err(err) => serde_json::json!({
            "status": "error",
            "code": "provider_error",
                "message": err.to_string(),
            }),
        }
    };

    if let Some(audit) = &chain_lifecycle_audit {
        let completed = response.get("status").and_then(|value| value.as_str()) == Some("ok");
        if let Err(err) = append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: SYSTEM_CAPSULE_ID,
                event_type: if completed {
                    "chain.node_lifecycle.completed"
                } else {
                    "chain.node_lifecycle.failed"
                },
                principal_id: &principal_id,
                session_id: &session_id,
                request_id: &audit.request_id,
                result: if completed { "completed" } else { "failed" },
                reason: &format!(
                    "System {} chain node lifecycle action {} for {}",
                    if completed { "completed" } else { "failed" },
                    audit.action,
                    audit.network
                ),
            },
        ) {
            return gateway_provider_error_response(
                &scheme,
                anyhow::anyhow!("chain node lifecycle audit failed: {}", err),
            );
        }
    }
    if let Some(request_id) = &library_audit {
        let completed = response.get("status").and_then(|value| value.as_str()) == Some("ok");
        if completed && library_operation_emits_events(&op) {
            crate::library::library_event_notifier().notify_waiters();
        }
        if let Err(err) = append_provider_effect_audit(
            &state.data_dir,
            ProviderEffectAuditInput {
                capsule_id: LIBRARY_CAPSULE_ID,
                event_type: if completed {
                    "object.provider.completed"
                } else {
                    "object.provider.failed"
                },
                principal_id: &principal_id,
                session_id: &session_id,
                request_id,
                result: if completed { "completed" } else { "failed" },
                reason: &format!(
                    "Library {} object provider operation {op}",
                    if completed { "completed" } else { "failed" }
                ),
            },
        ) {
            return gateway_provider_error_response(
                &scheme,
                anyhow::anyhow!("object provider audit failed: {}", err),
            );
        }
    }

    Json(response).into_response()
}

pub(super) fn provider_proxy_runtime_metadata_field(request: &serde_json::Value) -> Option<&str> {
    request
        .as_object()?
        .keys()
        .find(|key| {
            key.starts_with("_runtime")
                || matches!(key.as_str(), "connect_ticket" | "carrier_route" | "carrier")
        })
        .map(String::as_str)
}

fn library_operation_emits_events(op: &str) -> bool {
    matches!(
        op,
        "write"
            | "mkdir"
            | "rename"
            | "move"
            | "copy"
            | "trash"
            | "restore"
            | "delete_permanently"
            | "empty_trash"
            | "extract_archive"
            | "archive_extract_entries"
            | "compress_archive"
            | "publish"
            | "unpublish"
            | "repair"
            | "share"
    )
}

fn library_operation_needs_runtime_coordinator(op: &str) -> bool {
    matches!(op, "publish" | "unpublish" | "repair" | "sync")
}

fn library_request_targets_webspace(request: &serde_json::Value) -> bool {
    ["uri", "parent_uri", "target_uri", "target_parent_uri"]
        .iter()
        .filter_map(|field| request.get(field).and_then(|value| value.as_str()))
        .map(str::trim)
        .map(|value| value.trim_end_matches('/'))
        .any(|value| {
            value == "localhost://WebSpaces"
                || value
                    .strip_prefix("localhost://WebSpaces/")
                    .is_some_and(|rest| !rest.is_empty())
        })
}

struct LibraryEventsStreamState {
    registry: Arc<ProviderRegistry>,
    principal_id: String,
    cursor: String,
    initialized: bool,
}

async fn library_events_since_cursor(
    registry: &ProviderRegistry,
    principal_id: &str,
    cursor: &str,
) -> anyhow::Result<(Vec<serde_json::Value>, String)> {
    let response = registry
        .send_raw(
            "object",
            &serde_json::json!({
                "op": "events",
                "principal_id": principal_id,
                "limit": 256,
            }),
        )
        .await
        .map_err(|err| anyhow::anyhow!("object provider unavailable: {err}"))?;
    if response.get("status").and_then(|value| value.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("library events failed");
        anyhow::bail!("{message}");
    }
    let events = response
        .get("data")
        .and_then(|data| data.get("events"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let next_cursor = events
        .last()
        .and_then(|event| event.get("event_id"))
        .and_then(|value| value.as_str())
        .unwrap_or(cursor)
        .to_string();
    if cursor.is_empty() {
        return Ok((events, next_cursor));
    }
    let cursor_index = events.iter().position(|event| {
        event
            .get("event_id")
            .and_then(|value| value.as_str())
            .is_some_and(|event_id| event_id == cursor)
    });
    let filtered = if let Some(index) = cursor_index {
        events.into_iter().skip(index + 1).collect()
    } else {
        events
    };
    Ok((filtered, next_cursor))
}

fn library_events_sse_event(payload: serde_json::Value) -> SseEvent {
    let data = serde_json::to_string(&payload).unwrap_or_else(|_| {
        r#"{"schema":"elastos.library.events/v1","status":"error","events":[]}"#.to_string()
    });
    SseEvent::default().event("library-events").data(data)
}
