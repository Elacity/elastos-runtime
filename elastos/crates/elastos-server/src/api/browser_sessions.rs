use axum::extract::State;
use axum::http::{header::SET_COOKIE, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

use super::gateway::GatewayState;

#[derive(Debug, Deserialize)]
pub struct BrowserSessionPairRequestBody {
    pub app: String,
    pub display_name: String,
    #[serde(default)]
    pub device_label: String,
}

#[derive(Debug, Deserialize)]
pub struct BrowserSessionStatusRequestBody {
    pub app: String,
    pub request_id: String,
}

pub async fn browser_session_pair(
    State(state): State<GatewayState>,
    Json(body): Json<BrowserSessionPairRequestBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    match resolve_browser_session_app(&body.app) {
        Ok(BrowserSessionApp::RoomBrowser) => {
            match tokio::task::spawn_blocking(move || {
                let member_did = elastos_identity::load_or_create_did(&data_dir)
                    .ok()
                    .map(|(_signing_key, did)| did);
                crate::room_service::request_pairing(
                    &data_dir,
                    crate::room_service::PairRequestInput {
                        display_name: body.display_name,
                        device_label: body.device_label,
                        member_did,
                    },
                )
            })
            .await
            {
                Ok(Ok(output)) => Json(output).into_response(),
                Ok(Err(err)) => browser_session_error_response(err),
                Err(err) => browser_session_join_error_response(err),
            }
        }
        Err(err) => browser_session_error_response(err),
    }
}

pub async fn browser_session_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<BrowserSessionStatusRequestBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    match resolve_browser_session_app(&body.app) {
        Ok(BrowserSessionApp::RoomBrowser) => {
            match tokio::task::spawn_blocking(move || {
                crate::room_service::pairing_status(&data_dir, &body.request_id)
            })
            .await
            {
                Ok(Ok(output)) => browser_session_status_response(output, &headers),
                Ok(Err(err)) => browser_session_error_response(err),
                Err(err) => browser_session_join_error_response(err),
            }
        }
        Err(err) => browser_session_error_response(err),
    }
}

enum BrowserSessionApp {
    RoomBrowser,
}

fn resolve_browser_session_app(app: &str) -> anyhow::Result<BrowserSessionApp> {
    match app.trim() {
        "room-browser" => Ok(BrowserSessionApp::RoomBrowser),
        _ => anyhow::bail!("browser capsule not found"),
    }
}

fn browser_session_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("not an active member")
        || text.contains("no active room member DID")
        || text.contains("pairing is not allowed")
    {
        StatusCode::FORBIDDEN
    } else if text.contains("invalid or expired session") {
        StatusCode::UNAUTHORIZED
    } else if text.contains("must not be empty")
        || text.contains("characters or fewer")
        || text.contains("exceeds")
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
    mut output: crate::room_service::PairStatusOutput,
    headers: &HeaderMap,
) -> Response {
    if output.status != "approved" {
        return Json(output).into_response();
    }

    let Some(token) = output.token.take() else {
        return browser_session_error_response(anyhow::anyhow!(
            "approved status missing session token"
        ));
    };
    let secure = super::gateway::request_uses_tls(headers);
    let max_age_secs = output
        .expires_at
        .map(|expires_at| expires_at.saturating_sub(now_ts()))
        .unwrap_or(12 * 60 * 60);

    let mut response = Json(output).into_response();
    match super::gateway::set_room_session_cookie_header(&token, max_age_secs, secure) {
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
