use axum::extract::{Path, State};
use axum::http::{header::SET_COOKIE, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use super::gateway::GatewayState;

const BROWSER_SESSION_REQUEST_COOKIE: &str = "browser-session-request";
const BROWSER_SESSION_REQUEST_COOKIE_MAX_AGE_SECS: u64 = 10 * 60;

#[derive(Debug, Deserialize)]
pub struct BrowserSessionRequestBody {
    pub display_name: String,
    #[serde(default)]
    pub device_label: String,
    pub capabilities: Vec<String>,
}

pub async fn browser_session_request(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<BrowserSessionRequestBody>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_collaboration_browser_session_unsupported_response();
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let host_member_did = elastos_identity::load_or_create_did(&data_dir)
            .ok()
            .map(|(_signing_key, did)| did);
        crate::room_service::request_browser_access(
            &data_dir,
            crate::room_service::BrowserAccessRequestInput {
                display_name: body.display_name,
                device_label: body.device_label,
                host_member_did,
                capabilities: body.capabilities,
            },
        )
    })
    .await
    {
        Ok(Ok(output)) => {
            if let Ok(summary) = crate::room_service::load_summary(&state.data_dir) {
                let _ = crate::notifications::sync_room_notifications(&state.data_dir, &summary);
            }
            let secure = super::gateway::request_uses_tls(&headers);
            let mut response = Json(output.clone()).into_response();
            match set_browser_request_cookie_header(&output.request_id, secure) {
                Ok(cookie) => {
                    response.headers_mut().append(SET_COOKIE, cookie);
                    response
                }
                Err(err) => browser_session_error_response(err),
            }
        }
        Ok(Err(err)) => browser_session_error_response(err),
        Err(err) => browser_session_join_error_response(err),
    }
}

pub async fn browser_session_request_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_collaboration_browser_session_unsupported_response();
    }
    if super::gateway::cookie_value_from_headers(&headers, BROWSER_SESSION_REQUEST_COOKIE)
        .as_deref()
        != Some(request_id.as_str())
    {
        return (
            StatusCode::FORBIDDEN,
            "web guest request is not bound to this browser",
        )
            .into_response();
    }

    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::browser_access_status(&data_dir, &request_id)
    })
    .await
    {
        Ok(Ok(output)) => browser_session_status_response(output, &headers),
        Ok(Err(err)) => browser_session_error_response(err),
        Err(err) => browser_session_join_error_response(err),
    }
}

fn configured_collaboration_browser_session_unsupported_response() -> Response {
    (
        StatusCode::CONFLICT,
        "Configured collaboration Chat is available only through its signed Home projection.",
    )
        .into_response()
}

fn set_browser_request_cookie_header(
    request_id: &str,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{BROWSER_SESSION_REQUEST_COOKIE}={request_id}; Max-Age={BROWSER_SESSION_REQUEST_COOKIE_MAX_AGE_SECS}; Path=/api/browser/session/request; HttpOnly; SameSite=Lax"
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

fn browser_session_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("not an active member")
        || text.contains("no active room member DID")
        || text.contains("not part of this conversation")
        || text.contains("browser access is not allowed")
        || text.contains("web guest access is not allowed")
    {
        StatusCode::FORBIDDEN
    } else if text.contains("invalid or expired session") {
        StatusCode::UNAUTHORIZED
    } else if text.contains("must not be empty")
        || text.contains("characters or fewer")
        || text.contains("exceeds")
        || text.contains("unsupported browser session capability")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn browser_session_join_error_response(err: tokio::task::JoinError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("browser session task failed: {}", err),
    )
        .into_response()
}

fn browser_session_status_response(
    mut output: crate::room_service::BrowserAccessStatusOutput,
    headers: &HeaderMap,
) -> Response {
    if output.status != "approved" {
        return Json(output).into_response();
    }

    let Some(token) = output.token.take() else {
        return browser_session_error_response(anyhow::anyhow!(
            "approved status missing browser session token"
        ));
    };
    let secure = super::gateway::request_uses_tls(headers);
    let max_age_secs = output
        .expires_at
        .map(|expires_at| expires_at.saturating_sub(now_ts()))
        .unwrap_or(12 * 60 * 60);

    let mut response = Json(output).into_response();
    match super::gateway::set_browser_session_cookie_header(&token, max_age_secs, secure) {
        Ok(cookie) => {
            response.headers_mut().append(SET_COOKIE, cookie);
            response
        }
        Err(err) => browser_session_error_response(err),
    }
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
