use super::*;
use ed25519_dalek::{Signer as _, Verifier as _};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{mpsc, LazyLock, Mutex as StdMutex};

const ROOM_TRANSPORT_OUTBOUND_QUEUE: usize = 512;
const ROOM_TRANSPORT_OUTBOUND_BATCH: usize = 64;
const ROOM_TRANSPORT_REPLAY_OUTBOUND_BATCH: usize = 8;
const ROOM_TRANSPORT_REPLAY_LIMIT: usize = 128;
const ROOM_TRANSPORT_REPLAY_TICKS: u64 = 8;
const ROOM_TRANSPORT_IDLE_POLL_MS: u64 = 250;
const ROOM_TRANSPORT_ERROR_BACKOFF_MS: u64 = 1_000;
const ROOM_TRANSPORT_RECV_LIMIT: u64 = 512;
const ROOM_TRANSPORT_RECV_DRAIN_PAGES: usize = 8;
const ROOM_TRANSPORT_PEER_REFRESH_TICKS: u64 = 8;
const ROOM_TRANSPORT_BOOTSTRAP_CARRIER_TIMEOUT_SECS: u64 = 5;

static ROOM_TRANSPORT_BRIDGES: LazyLock<StdMutex<HashMap<PathBuf, Arc<RoomTransportBridge>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoomPollBody {
    #[serde(default)]
    since: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoomSendBody {
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomAccessPolicyBody {
    allow_guest_invites: bool,
    allow_member_invites: bool,
    allow_members_to_host_guests: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomMemberInviteBody {
    member_did: String,
    #[serde(default)]
    role: Option<crate::room_service::RoomRole>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomMemberRemoveBody {
    member_did: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomInviteRevokeBody {
    invite_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteCreateBody {
    #[serde(default)]
    issuer_gateway: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteJoinBody {
    invite: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteClaimBody {
    token: String,
    member_did: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ChatRoomJoinInviteClaimResponse {
    invite: crate::room_service::SignedRoomInviteEnvelope,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteAcceptanceBody {
    acceptance: crate::room_service::SignedRoomAcceptEnvelope,
}

#[derive(Debug, Serialize)]
pub(super) struct ChatRoomJoinInviteJoinResponse {
    status: String,
    room_title: String,
    issuer_gateway: String,
    member_did: String,
    invite_id: String,
}

#[derive(Debug, Serialize)]
struct ChatRoomGuestKickResponse {
    status: String,
    display_name: String,
    device_label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoomUploadStartBody {
    file_name: String,
    #[serde(default)]
    mime_type: String,
    size_bytes: u64,
}

pub(super) async fn room_service_summary(State(state): State<GatewayState>) -> Response {
    let data_dir = state.data_dir.clone();
    let summary_result =
        tokio::task::spawn_blocking(move || load_room_summary_with_identity(&data_dir)).await;
    match summary_result {
        Ok(Ok(mut summary)) => {
            summary.transport = if summary.local_runtime_did.is_some() {
                room_transport_view(&state, None).await
            } else {
                let topic = room_transport_topic(crate::room_service::room_slug());
                crate::room_service::RoomTransportView {
                    available: false,
                    connected_peer_count: 0,
                    topic: Some(topic),
                    status: Some(
                        "Carrier conversation sync inactive: local ElastOS identity is not set yet."
                            .to_string(),
                    ),
                }
            };
            Json(GatewayRoomSummary::from(summary)).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_summary(State(state): State<GatewayState>) -> Response {
    room_service_summary(State(state)).await
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct CarrierBootstrapQuery {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

pub(super) async fn gateway_carrier_bootstrap(
    State(state): State<GatewayState>,
    Query(query): Query<CarrierBootstrapQuery>,
) -> Response {
    let publisher_bootstrap = carrier_bootstrap_query_requests_publisher(&query);
    let bootstrap = if publisher_bootstrap {
        match gateway_provider_carrier_bootstrap(&state).await {
            Ok(bootstrap) => bootstrap,
            Err(response) => return response,
        }
    } else {
        match managed_runtime_carrier_bootstrap(state.data_dir.clone()).await {
            Some(bootstrap) => bootstrap,
            None => match gateway_provider_carrier_bootstrap(&state).await {
                Ok(bootstrap) => bootstrap,
                Err(response) => return response,
            },
        }
    };
    let did = elastos_identity::load_or_create_did(&state.data_dir)
        .ok()
        .map(|(_, did)| did)
        .unwrap_or_default();

    (
        StatusCode::OK,
        [(CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "schema": "elastos.carrier.bootstrap/v1",
            "transport": "carrier",
            "ticket": bootstrap.ticket,
            "node_id": bootstrap.node_id,
            "did": did,
            "role": if publisher_bootstrap { "publisher" } else { "runtime" },
            "generated_at": now_ts(),
        })),
    )
        .into_response()
}

fn carrier_bootstrap_query_requests_publisher(query: &CarrierBootstrapQuery) -> bool {
    query
        .role
        .as_deref()
        .or(query.scope.as_deref())
        .is_some_and(|value| matches!(value, "publisher" | "source" | "trusted-source"))
}

async fn gateway_provider_carrier_bootstrap(
    state: &GatewayState,
) -> Result<CarrierBootstrapTicket, Response> {
    let Some(registry) = state.provider_registry.as_ref().cloned() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Carrier bootstrap unavailable",
        )
            .into_response());
    };
    let body = match registry
        .send_raw("peer", &serde_json::json!({ "op": "get_ticket" }))
        .await
    {
        Ok(body) => body,
        Err(err) => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Carrier bootstrap unavailable: {err}"),
            )
                .into_response())
        }
    };
    if body.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("Carrier bootstrap unavailable");
        return Err((StatusCode::SERVICE_UNAVAILABLE, message.to_string()).into_response());
    }
    carrier_bootstrap_from_body(&body).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Carrier bootstrap unavailable: ticket missing",
        )
            .into_response()
    })
}

#[derive(Debug, Clone)]
struct CarrierBootstrapTicket {
    ticket: String,
    node_id: String,
}

fn carrier_bootstrap_from_body(body: &serde_json::Value) -> Option<CarrierBootstrapTicket> {
    let data = body.get("data").unwrap_or(&serde_json::Value::Null);
    let ticket = data
        .get("ticket")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    let node_id = data
        .get("node_id")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if ticket.is_empty() || node_id.is_empty() {
        return None;
    }
    Some(CarrierBootstrapTicket {
        ticket: ticket.to_string(),
        node_id: node_id.to_string(),
    })
}

async fn managed_runtime_carrier_bootstrap(
    data_dir: std::path::PathBuf,
) -> Option<CarrierBootstrapTicket> {
    if let Some(coords) = load_runtime_coords(&data_dir) {
        if let Some(bootstrap) = managed_runtime_carrier_bootstrap_from_coords(
            coords.api_url.clone(),
            coords.attach_secret.clone(),
        )
        .await
        {
            return Some(bootstrap);
        }
    }
    let coords = crate::runtime_control::ensure_runtime_for_chat(&data_dir)
        .await
        .ok()?;
    managed_runtime_carrier_bootstrap_from_coords(coords.api_url, coords.attach_secret).await
}

async fn managed_runtime_carrier_bootstrap_from_coords(
    api_url: String,
    attach_secret: String,
) -> Option<CarrierBootstrapTicket> {
    tokio::task::spawn_blocking(move || {
        let coords = GatewayRuntimeCoords {
            api_url,
            attach_secret,
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .ok()?;
        let client_token = attach_client_token_blocking(&client, &coords)?;
        let peer_cap = request_attached_capability_blocking(
            &client,
            &coords.api_url,
            &client_token,
            "elastos://peer/*",
            "execute",
        )?;
        let body = peer_provider_request_blocking(
            &client,
            &coords.api_url,
            &client_token,
            &peer_cap,
            "get_ticket",
            serde_json::json!({}),
        )
        .ok()?;
        carrier_bootstrap_from_body(&body)
    })
    .await
    .ok()
    .flatten()
}

pub(super) async fn chat_room_session_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let secure = request_uses_tls(&headers);
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || start_chat_room_session(&data_dir, &context)).await {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope.clone()).await;
            let mut response = Json(ChatRoomSessionStartResponse {
                status: "connected".to_string(),
                display_name: output.display_name,
                expires_at: output.expires_at,
            })
            .into_response();
            match set_room_session_cookie_header(&output.token, output.max_age_secs, secure) {
                Ok(cookie) => {
                    response.headers_mut().append(SET_COOKIE, cookie);
                    response
                }
                Err(err) => room_service_error_response(err),
            }
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_request_approve(
    State(state): State<GatewayState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let outcome = crate::room_service::approve_request(&data_dir, &request_id)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "web guest request not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_request_deny(
    State(state): State<GatewayState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let outcome =
            crate::room_service::deny_request(&data_dir, &request_id, "Denied from Chat.")?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "web guest request not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_guest_kick(
    State(state): State<GatewayState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let outcome = crate::room_service::revoke_guest_session_by_id(&data_dir, &session_id)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(ChatRoomGuestKickResponse {
            status: "kicked".to_string(),
            display_name: output.display_name,
            device_label: output.device_label,
        })
        .into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "guest session not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_access_policy_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomAccessPolicyBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output = crate::room_service::update_room_access_policy(
            &data_dir,
            crate::room_service::RoomAccessPolicyUpdateInput {
                actor_did,
                allow_guest_invites: body.allow_guest_invites,
                allow_member_invites: body.allow_member_invites,
                allow_members_to_host_guests: body.allow_members_to_host_guests,
            },
        )?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_member_invite(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomMemberInviteBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output = crate::room_service::invite_room_member(
            &data_dir,
            crate::room_service::RoomInviteInput {
                actor_did,
                invited_did: body.member_did,
                role: body.role.unwrap_or(crate::room_service::RoomRole::Member),
            },
        )?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_member_remove(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomMemberRemoveBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output = crate::room_service::remove_room_member(
            &data_dir,
            crate::room_service::RoomMemberRemoveInput {
                actor_did,
                member_did: body.member_did,
            },
        )?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "room member not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_invite_revoke(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomInviteRevokeBody>,
) -> Response {
    if let Err(err) = require_home_launch_token(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        let output =
            crate::room_service::revoke_room_invite(&data_dir, &actor_did, &body.invite_id)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(output)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "room invite not found").into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_join_invite_create(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomJoinInviteCreateBody>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let issuer_gateway = match body
        .issuer_gateway
        .or_else(|| chat_room_invite_gateway_origin(&headers))
    {
        Some(value) => value,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "conversation invite needs a gateway origin",
            )
                .into_response()
        }
    };
    let data_dir = state.data_dir.clone();
    let inviter_profile =
        home_profile_card_summary_for_context(&data_dir, &context).map(room_profile_card_from_home);
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir)?;
        crate::room_service::export_room_join_invite(
            &data_dir,
            crate::room_service::RoomJoinInviteInput {
                actor_did,
                issuer_gateway,
                inviter_profile,
            },
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_join_invite_claim(
    State(state): State<GatewayState>,
    Json(body): Json<ChatRoomJoinInviteClaimBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::claim_room_join_invite(&data_dir, &body.token, &body.member_did)
    })
    .await
    {
        Ok(Ok(invite)) => Json(ChatRoomJoinInviteClaimResponse { invite }).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_join_invite_acceptance(
    State(state): State<GatewayState>,
    Json(body): Json<ChatRoomJoinInviteAcceptanceBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let bytes = serde_json::to_vec(&body.acceptance)?;
        let member = crate::room_service::import_room_acceptance_envelope(&data_dir, &bytes)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(member)
    })
    .await
    {
        Ok(Ok(member)) => Json(member).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_join_invite_join(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<ChatRoomJoinInviteJoinBody>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };

    let token = match crate::room_service::room_join_invite_token_from_input(&body.invite) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let (join_envelope, _) = match crate::room_service::decode_room_join_invite_token(&token) {
        Ok(decoded) => decoded,
        Err(err) => return room_service_error_response(err),
    };
    let issuer_gateway = join_envelope.payload.issuer_gateway.clone();
    let claim_url = format!(
        "{}/api/apps/chat-room/invites/claim",
        issuer_gateway.trim_end_matches('/')
    );
    let acceptance_url = format!(
        "{}/api/apps/chat-room/invites/acceptance",
        issuer_gateway.trim_end_matches('/')
    );

    let data_dir = state.data_dir.clone();
    let local_did_result = tokio::task::spawn_blocking(move || {
        elastos_identity::load_or_create_did(&data_dir).map(|(_, did)| did)
    })
    .await;
    let local_did = match local_did_result {
        Ok(Ok(did)) => did,
        Ok(Err(err)) => return room_service_error_response(err),
        Err(err) => return room_service_join_error_response(err),
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(err) => return room_service_error_response(anyhow::anyhow!(err)),
    };
    let claim_response = match client
        .post(&claim_url)
        .json(&ChatRoomJoinInviteClaimBody {
            token: token.clone(),
            member_did: local_did.clone(),
        })
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return room_service_error_response(anyhow::anyhow!(err)),
    };
    let claim_response = match claim_response.error_for_status() {
        Ok(response) => response,
        Err(err) => return room_service_error_response(anyhow::anyhow!(err)),
    };
    let claim: ChatRoomJoinInviteClaimResponse = match claim_response.json().await {
        Ok(value) => value,
        Err(err) => return room_service_error_response(anyhow::anyhow!(err)),
    };

    let data_dir = state.data_dir.clone();
    let member_profile =
        home_profile_card_summary_for_context(&data_dir, &context).map(room_profile_card_from_home);
    let invite = claim.invite.clone();
    let local_did_for_accept = local_did.clone();
    let local_accept_result = tokio::task::spawn_blocking(move || {
        let invite_bytes = serde_json::to_vec(&invite)?;
        let imported = crate::room_service::import_room_invite_envelope(&data_dir, &invite_bytes)?;
        let member = crate::room_service::accept_room_invite(
            &data_dir,
            crate::room_service::RoomInviteAcceptInput {
                actor_did: local_did_for_accept,
                invite_id: imported.invite_id.clone(),
            },
        )?;
        let acceptance = crate::room_service::export_room_acceptance_envelope_with_profile(
            &data_dir,
            &imported.invite_id,
            member_profile,
        )?;
        Ok::<_, anyhow::Error>((imported, member, acceptance))
    })
    .await;
    let (imported, member, acceptance) = match local_accept_result {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => return room_service_error_response(err),
        Err(err) => return room_service_join_error_response(err),
    };

    let acceptance_response = match client
        .post(&acceptance_url)
        .json(&ChatRoomJoinInviteAcceptanceBody { acceptance })
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return room_service_error_response(anyhow::anyhow!(err)),
    };
    if let Err(err) = acceptance_response.error_for_status() {
        return room_service_error_response(anyhow::anyhow!(err));
    }

    Json(ChatRoomJoinInviteJoinResponse {
        status: "joined".to_string(),
        room_title: join_envelope.payload.room_title,
        issuer_gateway,
        member_did: member.member_did,
        invite_id: imported.invite_id,
    })
    .into_response()
}

pub(super) fn chat_room_invite_gateway_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| *value == "http" || *value == "https")
        .unwrap_or_else(|| {
            if host.starts_with("127.0.0.1")
                || host.starts_with("localhost")
                || host.starts_with("[::1]")
            {
                "http"
            } else {
                "https"
            }
        });
    Some(format!("{proto}://{host}"))
}

pub(super) fn room_profile_card_from_home(
    card: HomeProfileCardSummary,
) -> crate::room_service::RoomProfileCardView {
    crate::room_service::RoomProfileCardView {
        schema: card.schema,
        profile_id: card.profile_id,
        display_name: card.display_name,
        handle: card.handle,
        updated_at: card.updated_at,
    }
}

fn load_room_summary_with_identity(
    data_dir: &std::path::Path,
) -> anyhow::Result<crate::room_service::RoomSummary> {
    let identity = room_service_runtime_identity_profile(data_dir);
    let mut summary = crate::room_service::load_summary(data_dir)?;
    if let Ok(hosted) = crate::browser_app_hosts::load_browser_app_hosted_endpoint(
        data_dir,
        crate::room_service::room_slug(),
    ) {
        summary.canonical_hosted_guest_url = hosted.canonical_url;
        summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
    }
    let access = crate::room_service::local_runtime_access(data_dir, identity.did.as_deref())?;
    apply_room_access(&mut summary, access);
    Ok(summary)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayIdentityProfile {
    pub did: Option<String>,
}

pub(crate) fn room_service_runtime_identity_profile(
    data_dir: &std::path::Path,
) -> GatewayIdentityProfile {
    let did = load_existing_gateway_runtime_did(data_dir);
    GatewayIdentityProfile { did }
}

struct ChatRoomSessionGrant {
    token: String,
    display_name: String,
    expires_at: u64,
    max_age_secs: u64,
    transport_envelope: Option<crate::room_service::RoomObjectEnvelope>,
}

fn start_chat_room_session(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<ChatRoomSessionGrant> {
    let identity = load_gateway_identity_summary_for_context(data_dir, context);
    let did = identity
        .device_did
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("local ElastOS identity is unavailable"))?;
    let handle = identity
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("proof-bound passkey handle unavailable"))?;
    let output = crate::room_service::start_local_principal_runtime_session_with_transport(
        data_dir,
        did,
        &context.principal_id,
        handle,
        "ElastOS shell",
    )?;
    Ok(ChatRoomSessionGrant {
        max_age_secs: output.session.expires_at.saturating_sub(now_ts()),
        token: output.session.token,
        display_name: output.session.display_name,
        expires_at: output.session.expires_at,
        transport_envelope: output.transport_envelope,
    })
}

pub(super) fn ensure_local_principal_room_session(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<()> {
    start_chat_room_session(data_dir, context).map(|_| ())
}

pub(super) fn ensure_local_room_owner_or_actor(
    data_dir: &std::path::Path,
) -> anyhow::Result<String> {
    let identity = load_gateway_identity_summary(data_dir);
    let did = identity
        .device_did
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("local ElastOS identity is unavailable"))?;
    let control = crate::room_service::load_room_control(data_dir)?;
    if control.owner_did.is_none() {
        let _ = crate::room_service::seed_room_owner(
            data_dir,
            crate::room_service::RoomOwnerSeedInput {
                owner_did: did.clone(),
                title: "Chat".to_string(),
            },
        )?;
    }
    Ok(did)
}

fn attach_client_token_blocking(
    client: &reqwest::blocking::Client,
    coords: &GatewayRuntimeCoords,
) -> Option<String> {
    let body: serde_json::Value = client
        .post(format!("{}/api/auth/attach", coords.api_url))
        .json(&serde_json::json!({
            "secret": coords.attach_secret,
            "scope": "client",
        }))
        .send()
        .ok()?
        .json()
        .ok()?;
    serde_json::from_value::<GatewayAttachResponse>(body)
        .ok()
        .map(|resp| resp.token)
}

fn request_attached_capability_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    resource: &str,
    action: &str,
) -> Option<String> {
    let body: serde_json::Value = client
        .post(format!("{}/api/capability/request", api))
        .header("Authorization", format!("Bearer {}", client_token))
        .json(&serde_json::json!({
            "resource": resource,
            "action": action,
        }))
        .send()
        .ok()?
        .json()
        .ok()?;

    if let Some(token) = body.get("token").and_then(|t| t.as_str()) {
        return Some(token.to_string());
    }

    let request_id = body.get("request_id").and_then(|r| r.as_str())?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let status: serde_json::Value = client
            .get(format!("{}/api/capability/request/{}", api, request_id))
            .header("Authorization", format!("Bearer {}", client_token))
            .send()
            .ok()?
            .json()
            .ok()?;
        if let Some(token) = status.get("token").and_then(|t| t.as_str()) {
            return Some(token.to_string());
        }
        match status.get("status").and_then(|s| s.as_str()) {
            Some("denied") | Some("expired") => return None,
            _ => {}
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct GatewayGossipMessage {
    sender_id: String,
    content: String,
    ts: u64,
    #[serde(default)]
    signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CarrierBootstrapDocument {
    #[serde(default)]
    ticket: String,
    #[serde(default)]
    connect_ticket: String,
}

struct AttachedRoomRuntimeBlocking {
    client: reqwest::blocking::Client,
    api_url: String,
    client_token: String,
    peer_cap: String,
    did: String,
    room_signing_key: ed25519_dalek::SigningKey,
}

struct RoomTransportBridge {
    outbound: mpsc::SyncSender<crate::room_service::RoomObjectEnvelope>,
    last_view: Arc<StdMutex<crate::room_service::RoomTransportView>>,
}

impl RoomTransportBridge {
    fn view(&self) -> crate::room_service::RoomTransportView {
        self.last_view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_view(&self, view: crate::room_service::RoomTransportView) {
        *self
            .last_view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = view;
    }

    fn enqueue(&self, envelope: crate::room_service::RoomObjectEnvelope) -> anyhow::Result<()> {
        self.outbound
            .try_send(envelope)
            .map_err(|err| anyhow::anyhow!("{err}"))
    }
}

struct RoomTransportBridgeState {
    runtime: Option<AttachedRoomRuntimeBlocking>,
    joined: bool,
    connected_peer_count: usize,
    tick: u64,
    send_retry_tick: u64,
    pending_outbound: VecDeque<crate::room_service::RoomObjectEnvelope>,
    queued_event_ids: HashSet<String>,
    replay_event_ids: HashSet<String>,
    sent_event_ticks: HashMap<String, u64>,
    last_replay_tick: Option<u64>,
}

struct RoomTransportRecvBatch {
    messages: Vec<GatewayGossipMessage>,
    scanned: usize,
}

enum RoomTransportSendOutcome {
    Delivered,
    LocalOnly,
}

pub(super) async fn room_transport_view(
    state: &GatewayState,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> crate::room_service::RoomTransportView {
    let topic = room_transport_topic(crate::room_service::room_slug());
    let bridge = match ensure_room_transport_bridge(state).await {
        Ok(bridge) => bridge,
        Err(err) => return room_transport_error_view(&topic, &err.to_string()),
    };
    if let Some(envelope) = outbound {
        if let Err(err) = bridge.enqueue(envelope) {
            return room_transport_error_view(
                &topic,
                &format!("room transport outbound queue unavailable: {err}"),
            );
        }
    }
    bridge.view()
}

async fn ensure_room_transport_bridge(
    state: &GatewayState,
) -> anyhow::Result<Arc<RoomTransportBridge>> {
    let data_dir = state.data_dir.clone();
    if let Some(bridge) = room_transport_bridge_for_data_dir(&data_dir) {
        return Ok(bridge);
    }
    crate::runtime_control::ensure_runtime_for_home(&data_dir)
        .await
        .map_err(|err| anyhow::anyhow!("managed runtime could not start: {err}"))?;
    if let Some(bridge) = room_transport_bridge_for_data_dir(&data_dir) {
        return Ok(bridge);
    }
    let mut bridges = ROOM_TRANSPORT_BRIDGES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(bridge) = bridges.get(&data_dir) {
        return Ok(bridge.clone());
    }
    let bridge = spawn_room_transport_bridge(data_dir.clone())?;
    bridges.insert(data_dir, bridge.clone());
    Ok(bridge)
}

fn room_transport_bridge_for_data_dir(
    data_dir: &std::path::Path,
) -> Option<Arc<RoomTransportBridge>> {
    ROOM_TRANSPORT_BRIDGES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(data_dir)
        .cloned()
}

fn spawn_room_transport_bridge(data_dir: PathBuf) -> anyhow::Result<Arc<RoomTransportBridge>> {
    let topic = room_transport_topic(crate::room_service::room_slug());
    let (outbound, inbound) = mpsc::sync_channel(ROOM_TRANSPORT_OUTBOUND_QUEUE);
    let bridge = Arc::new(RoomTransportBridge {
        outbound,
        last_view: Arc::new(StdMutex::new(crate::room_service::RoomTransportView {
            available: false,
            connected_peer_count: 0,
            topic: Some(topic.clone()),
            status: Some("Carrier room sync starting.".to_string()),
        })),
    });
    let worker_bridge = bridge.clone();
    std::thread::Builder::new()
        .name(format!(
            "room-transport-{}",
            crate::room_service::room_slug()
        ))
        .spawn(move || run_room_transport_bridge(data_dir, inbound, worker_bridge))
        .map_err(|err| anyhow::anyhow!("failed to spawn room transport bridge: {err}"))?;
    Ok(bridge)
}

fn room_transport_topic(room_slug: &str) -> String {
    format!("__elastos_internal/room-sync-v1/{room_slug}")
}

fn room_transport_error_view(topic: &str, detail: &str) -> crate::room_service::RoomTransportView {
    crate::room_service::RoomTransportView {
        available: false,
        connected_peer_count: 0,
        topic: Some(topic.to_string()),
        status: Some(format!("Carrier conversation sync unavailable: {detail}")),
    }
}

fn run_room_transport_bridge(
    data_dir: PathBuf,
    inbound: mpsc::Receiver<crate::room_service::RoomObjectEnvelope>,
    bridge: Arc<RoomTransportBridge>,
) {
    let topic = room_transport_topic(crate::room_service::room_slug());
    let mut state = RoomTransportBridgeState {
        runtime: None,
        joined: false,
        connected_peer_count: 0,
        tick: 0,
        send_retry_tick: 0,
        pending_outbound: VecDeque::new(),
        queued_event_ids: HashSet::new(),
        replay_event_ids: HashSet::new(),
        sent_event_ticks: HashMap::new(),
        last_replay_tick: None,
    };

    loop {
        let mut live_envelopes = Vec::with_capacity(ROOM_TRANSPORT_OUTBOUND_BATCH);
        match inbound.recv_timeout(Duration::from_millis(ROOM_TRANSPORT_IDLE_POLL_MS)) {
            Ok(envelope) => {
                live_envelopes.push(envelope);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for _ in 0..ROOM_TRANSPORT_OUTBOUND_BATCH {
            match inbound.try_recv() {
                Ok(envelope) => {
                    live_envelopes.push(envelope);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        for envelope in live_envelopes.into_iter().rev() {
            let _ = queue_live_room_transport_outbound(&mut state, envelope);
        }

        match sync_room_transport_bridge_tick(&data_dir, &topic, &mut state) {
            Ok(view) => bridge.set_view(view),
            Err(err) => {
                state.runtime = None;
                state.joined = false;
                state.connected_peer_count = 0;
                bridge.set_view(room_transport_error_view(&topic, &err.to_string()));
                std::thread::sleep(Duration::from_millis(ROOM_TRANSPORT_ERROR_BACKOFF_MS));
            }
        }
    }
}

fn queue_room_transport_outbound(
    state: &mut RoomTransportBridgeState,
    envelope: crate::room_service::RoomObjectEnvelope,
) -> bool {
    if state
        .sent_event_ticks
        .get(&envelope.event_id)
        .is_some_and(|last| state.tick.saturating_sub(*last) < ROOM_TRANSPORT_REPLAY_TICKS)
    {
        return false;
    }
    if !state.queued_event_ids.insert(envelope.event_id.clone()) {
        return false;
    }
    if state.pending_outbound.len() >= ROOM_TRANSPORT_OUTBOUND_QUEUE {
        if let Some(dropped) = state.pending_outbound.pop_front() {
            state.queued_event_ids.remove(&dropped.event_id);
            state.replay_event_ids.remove(&dropped.event_id);
        }
    }
    state.replay_event_ids.insert(envelope.event_id.clone());
    state.pending_outbound.push_back(envelope);
    true
}

fn queue_live_room_transport_outbound(
    state: &mut RoomTransportBridgeState,
    envelope: crate::room_service::RoomObjectEnvelope,
) -> bool {
    if state
        .sent_event_ticks
        .get(&envelope.event_id)
        .is_some_and(|last| state.tick.saturating_sub(*last) < ROOM_TRANSPORT_REPLAY_TICKS)
    {
        return false;
    }
    if !state.queued_event_ids.insert(envelope.event_id.clone()) {
        if state.replay_event_ids.remove(&envelope.event_id) {
            if let Some(index) = state
                .pending_outbound
                .iter()
                .position(|queued| queued.event_id == envelope.event_id)
            {
                let Some(queued) = state.pending_outbound.remove(index) else {
                    return false;
                };
                state.pending_outbound.push_front(queued);
                return true;
            }
        }
        return false;
    }
    if state.pending_outbound.len() >= ROOM_TRANSPORT_OUTBOUND_QUEUE {
        if let Some(dropped) = state.pending_outbound.pop_back() {
            state.queued_event_ids.remove(&dropped.event_id);
            state.replay_event_ids.remove(&dropped.event_id);
        }
    }
    state.pending_outbound.push_front(envelope);
    true
}

fn pop_delivered_room_transport_outbound(
    state: &mut RoomTransportBridgeState,
    envelope: crate::room_service::RoomObjectEnvelope,
) {
    state.queued_event_ids.remove(&envelope.event_id);
    let was_replay = state.replay_event_ids.remove(&envelope.event_id);
    if was_replay {
        state.sent_event_ticks.insert(envelope.event_id, state.tick);
    }
    if state.sent_event_ticks.len() > ROOM_TRANSPORT_OUTBOUND_QUEUE * 4 {
        let cutoff = state.tick.saturating_sub(ROOM_TRANSPORT_REPLAY_TICKS * 8);
        state.sent_event_ticks.retain(|_, tick| *tick >= cutoff);
    }
}

fn rotate_local_only_room_transport_outbound(
    state: &mut RoomTransportBridgeState,
    envelope: crate::room_service::RoomObjectEnvelope,
) {
    if state.replay_event_ids.contains(&envelope.event_id) {
        state.pending_outbound.push_back(envelope);
    } else {
        state.pending_outbound.push_front(envelope);
    }
}

fn room_transport_send_batch_limit(state: &RoomTransportBridgeState) -> usize {
    let Some(envelope) = state.pending_outbound.front() else {
        return 0;
    };
    if state.replay_event_ids.contains(&envelope.event_id) {
        if state.tick % ROOM_TRANSPORT_REPLAY_TICKS != 0 {
            return 0;
        }
        ROOM_TRANSPORT_REPLAY_OUTBOUND_BATCH
    } else {
        ROOM_TRANSPORT_OUTBOUND_BATCH
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod room_transport_queue_tests {
    use super::*;

    fn test_envelope(event_id: &str) -> crate::room_service::RoomObjectEnvelope {
        crate::room_service::RoomObjectEnvelope {
            schema: "elastos.chat-room.object/v1".to_string(),
            room_slug: crate::room_service::room_slug().to_string(),
            event_id: event_id.to_string(),
            sender: "Tester".to_string(),
            sender_member_did: "did:key:z6MkiTest".to_string(),
            kind: crate::room_service::ConversationObjectKind::Text,
            body: Some(event_id.to_string()),
            emoji: None,
            link: None,
            attachment: None,
            attachment_bytes_b64: None,
            created_at: 1,
        }
    }

    fn test_state() -> RoomTransportBridgeState {
        RoomTransportBridgeState {
            runtime: None,
            joined: false,
            connected_peer_count: 0,
            tick: 0,
            send_retry_tick: 0,
            pending_outbound: VecDeque::new(),
            queued_event_ids: HashSet::new(),
            replay_event_ids: HashSet::new(),
            sent_event_ticks: HashMap::new(),
            last_replay_tick: None,
        }
    }

    #[test]
    fn live_room_transport_outbound_preempts_replay_backlog_without_reordering_live_fifo() {
        let mut state = test_state();
        assert!(queue_room_transport_outbound(
            &mut state,
            test_envelope("replay-1")
        ));
        assert!(queue_room_transport_outbound(
            &mut state,
            test_envelope("replay-2")
        ));

        for envelope in [test_envelope("live-1"), test_envelope("live-2")]
            .into_iter()
            .rev()
        {
            assert!(queue_live_room_transport_outbound(&mut state, envelope));
        }

        let queued = state
            .pending_outbound
            .iter()
            .map(|envelope| envelope.event_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(queued, vec!["live-1", "live-2", "replay-1", "replay-2"]);
    }

    #[test]
    fn live_room_transport_outbound_drops_replay_tail_when_queue_is_full() {
        let mut state = test_state();
        for index in 0..ROOM_TRANSPORT_OUTBOUND_QUEUE {
            assert!(queue_room_transport_outbound(
                &mut state,
                test_envelope(&format!("replay-{index}"))
            ));
        }

        assert!(queue_live_room_transport_outbound(
            &mut state,
            test_envelope("live-now")
        ));

        assert_eq!(state.pending_outbound.len(), ROOM_TRANSPORT_OUTBOUND_QUEUE);
        assert_eq!(state.pending_outbound[0].event_id, "live-now");
        assert!(state.queued_event_ids.contains("live-now"));
        assert!(state.queued_event_ids.contains("replay-0"));
        assert!(!state
            .queued_event_ids
            .contains(&format!("replay-{}", ROOM_TRANSPORT_OUTBOUND_QUEUE - 1)));
        assert!(!state
            .replay_event_ids
            .contains(&format!("replay-{}", ROOM_TRANSPORT_OUTBOUND_QUEUE - 1)));
    }

    #[test]
    fn live_room_transport_outbound_promotes_existing_replay_duplicate() {
        let mut state = test_state();
        assert!(queue_room_transport_outbound(
            &mut state,
            test_envelope("replay-1")
        ));
        assert!(queue_room_transport_outbound(
            &mut state,
            test_envelope("fresh")
        ));

        assert!(queue_live_room_transport_outbound(
            &mut state,
            test_envelope("fresh")
        ));

        let queued = state
            .pending_outbound
            .iter()
            .map(|envelope| envelope.event_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(queued, vec!["fresh", "replay-1"]);
        assert!(!state.replay_event_ids.contains("fresh"));
        assert_eq!(
            room_transport_send_batch_limit(&state),
            ROOM_TRANSPORT_OUTBOUND_BATCH
        );
    }

    #[test]
    fn replay_room_transport_outbound_uses_background_batch_limit() {
        let mut state = test_state();
        assert!(queue_room_transport_outbound(
            &mut state,
            test_envelope("replay-only")
        ));

        assert_eq!(
            room_transport_send_batch_limit(&state),
            ROOM_TRANSPORT_REPLAY_OUTBOUND_BATCH
        );
    }

    #[test]
    fn replay_room_transport_outbound_yields_between_replay_ticks() {
        let mut state = test_state();
        state.tick = 1;
        assert!(queue_room_transport_outbound(
            &mut state,
            test_envelope("replay-only")
        ));

        assert_eq!(room_transport_send_batch_limit(&state), 0);

        assert!(queue_live_room_transport_outbound(
            &mut state,
            test_envelope("fresh")
        ));
        assert_eq!(
            room_transport_send_batch_limit(&state),
            ROOM_TRANSPORT_OUTBOUND_BATCH
        );
    }

    #[test]
    fn live_delivery_stays_eligible_for_one_history_replay() {
        let mut state = test_state();
        let live = test_envelope("maybe-missed");
        assert!(queue_live_room_transport_outbound(&mut state, live.clone()));

        state.tick = 2;
        pop_delivered_room_transport_outbound(&mut state, live.clone());
        assert!(queue_room_transport_outbound(&mut state, live.clone()));

        state.tick = ROOM_TRANSPORT_REPLAY_TICKS;
        pop_delivered_room_transport_outbound(&mut state, live.clone());
        assert!(!queue_room_transport_outbound(&mut state, live));
    }

    #[test]
    fn replay_room_transport_outbound_skips_history_when_bootstrap_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let (_, did) = elastos_identity::load_or_create_did(dir.path()).unwrap();
        crate::room_service::seed_room_owner(
            dir.path(),
            crate::room_service::RoomOwnerSeedInput {
                owner_did: did.clone(),
                title: "Bootstrap Replay Room".to_string(),
            },
        )
        .unwrap();
        let session =
            crate::room_service::start_local_runtime_session(dir.path(), &did, "Owner", "Home")
                .unwrap();
        let appended =
            crate::room_service::append_object_with_transport(dir.path(), &session.token, "hello")
                .unwrap();
        assert!(appended.transport_envelope.is_some());
        crate::sources::save_trusted_sources(
            dir.path(),
            &crate::sources::TrustedSourcesConfig {
                schema: "elastos.trusted-sources/v1".to_string(),
                default_source: "default".to_string(),
                sources: vec![crate::sources::TrustedSource {
                    name: "default".to_string(),
                    publisher_dids: vec![],
                    channel: "stable".to_string(),
                    discovery_uri: String::new(),
                    connect_ticket: "trusted-source-ticket".to_string(),
                    gateways: vec![],
                    install_path: String::new(),
                    installed_version: String::new(),
                    head_cid: String::new(),
                    publisher_node_id: "trusted-source-peer".to_string(),
                    ipns_name: String::new(),
                }],
            },
        )
        .unwrap();

        let mut state = test_state();
        let queued = replay_recent_room_transport_outbound(dir.path(), &did, &mut state).unwrap();

        assert_eq!(queued, 0);
        assert!(state.pending_outbound.is_empty());
        assert!(state.replay_event_ids.is_empty());
        assert_eq!(state.last_replay_tick, Some(0));
    }
}

fn replay_recent_room_transport_outbound(
    data_dir: &std::path::Path,
    sender_member_did: &str,
    state: &mut RoomTransportBridgeState,
) -> anyhow::Result<usize> {
    let should_replay = state
        .last_replay_tick
        .map(|last| state.tick.saturating_sub(last) >= ROOM_TRANSPORT_REPLAY_TICKS)
        .unwrap_or(true);
    if !should_replay {
        return Ok(0);
    }
    state.last_replay_tick = Some(state.tick);
    if room_transport_bootstrap_ticket(data_dir).is_some() {
        return Ok(0);
    }
    let envelopes = crate::room_service::recent_local_room_object_envelopes(
        data_dir,
        sender_member_did,
        ROOM_TRANSPORT_REPLAY_LIMIT,
    )?;
    let mut queued = 0usize;
    for envelope in envelopes {
        if queue_room_transport_outbound(state, envelope) {
            queued += 1;
        }
    }
    Ok(queued)
}

fn sync_room_transport_bridge_tick(
    data_dir: &std::path::Path,
    topic: &str,
    state: &mut RoomTransportBridgeState,
) -> anyhow::Result<crate::room_service::RoomTransportView> {
    if state.runtime.is_none() {
        state.runtime = Some(attach_room_runtime_blocking(data_dir)?);
        state.joined = false;
        state.connected_peer_count = 0;
    }
    let runtime_did = state
        .runtime
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?
        .did
        .clone();
    let access = crate::room_service::local_runtime_access(data_dir, Some(&runtime_did))?;
    if access.member_role.is_none() && !access.browser_access_allowed {
        return Ok(crate::room_service::RoomTransportView {
            available: false,
            connected_peer_count: 0,
            topic: Some(topic.to_string()),
            status: Some(access.block_reason.unwrap_or_else(|| {
                "Carrier conversation sync inactive: this device is not part of the conversation."
                    .to_string()
            })),
        });
    }

    if !state.joined {
        {
            let runtime = state
                .runtime
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
            join_room_transport_blocking(runtime, data_dir, topic)?;
        }
        state.joined = true;
        state.connected_peer_count = {
            let runtime = state
                .runtime
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
            list_room_transport_peers_blocking(runtime, topic)?.len()
        };
    } else if state.tick % ROOM_TRANSPORT_PEER_REFRESH_TICKS == 0 {
        state.connected_peer_count = {
            let runtime = state
                .runtime
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
            list_room_transport_peers_blocking(runtime, topic)?.len()
        };
        if state.connected_peer_count == 0 {
            {
                let runtime = state
                    .runtime
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
                join_room_transport_blocking(runtime, data_dir, topic)?;
            }
            state.connected_peer_count = {
                let runtime = state
                    .runtime
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
                list_room_transport_peers_blocking(runtime, topic)?.len()
            };
        }
    }

    let (mut imported, mut dropped) = {
        let runtime = state
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
        drain_room_transport_inbound_blocking(data_dir, runtime, topic)?
    };
    if state.tick % ROOM_TRANSPORT_PEER_REFRESH_TICKS == 0 {
        let runtime = state
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
        match pull_room_transport_bootstrap_blocking(data_dir, runtime, topic) {
            Ok((source_imported, source_dropped)) => {
                imported += source_imported;
                dropped += source_dropped;
            }
            Err(err) => tracing::debug!("room transport trusted-source pull failed: {}", err),
        }
    }

    let mut replayed = 0usize;
    if state.pending_outbound.is_empty() {
        replayed = replay_recent_room_transport_outbound(data_dir, &runtime_did, state)?;
    }

    let mut delivered = 0usize;
    let mut waiting_for_peer = false;
    if !state.pending_outbound.is_empty()
        && (state.connected_peer_count > 0 || state.tick >= state.send_retry_tick)
    {
        let send_count = room_transport_send_batch_limit(state).min(state.pending_outbound.len());
        for _ in 0..send_count {
            let Some(envelope) = state.pending_outbound.pop_front() else {
                break;
            };
            let send_outcome = {
                let runtime = state
                    .runtime
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not attached"))?;
                send_room_object_envelope_blocking(data_dir, runtime, topic, &envelope)?
            };
            match send_outcome {
                RoomTransportSendOutcome::Delivered => {
                    pop_delivered_room_transport_outbound(state, envelope);
                    delivered += 1;
                    state.send_retry_tick = state.tick;
                }
                RoomTransportSendOutcome::LocalOnly => {
                    state.connected_peer_count = 0;
                    waiting_for_peer = true;
                    state.send_retry_tick =
                        state.tick.saturating_add(ROOM_TRANSPORT_PEER_REFRESH_TICKS);
                    rotate_local_only_room_transport_outbound(state, envelope);
                    break;
                }
            }
        }
    } else if !state.pending_outbound.is_empty() {
        waiting_for_peer = true;
    }
    state.tick = state.tick.wrapping_add(1);

    let mut status = if state.connected_peer_count > 0 {
        format!(
            "Carrier conversation sync connected to {} ElastOS peer{}.",
            state.connected_peer_count,
            if state.connected_peer_count == 1 {
                ""
            } else {
                "s"
            }
        )
    } else {
        "Carrier conversation sync ready; waiting for another ElastOS peer.".to_string()
    };
    if waiting_for_peer {
        status.push_str(&format!(
            " {} outbound item{} queued.",
            state.pending_outbound.len(),
            if state.pending_outbound.len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if delivered > 0 {
        status.push_str(&format!(
            " Sent {} new item{}.",
            delivered,
            if delivered == 1 { "" } else { "s" }
        ));
    }
    if replayed > 0 {
        status.push_str(&format!(
            " Replayed {} local item{} from room history.",
            replayed,
            if replayed == 1 { "" } else { "s" }
        ));
    }
    if imported > 0 {
        status.push_str(&format!(
            " Imported {} new message{}.",
            imported,
            if imported == 1 { "" } else { "s" }
        ));
    }
    if dropped > 0 {
        status.push_str(&format!(
            " Ignored {} invalid item{}.",
            dropped,
            if dropped == 1 { "" } else { "s" }
        ));
    }

    Ok(crate::room_service::RoomTransportView {
        available: true,
        connected_peer_count: state.connected_peer_count,
        topic: Some(topic.to_string()),
        status: Some(status),
    })
}

fn join_room_transport_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    data_dir: &std::path::Path,
    topic: &str,
) -> anyhow::Result<()> {
    let mut join_mode = "dht";
    let mut bootstrap_peers = Vec::new();
    if let Some(ticket) = room_transport_bootstrap_ticket(data_dir) {
        match remember_room_transport_bootstrap_blocking(runtime, &ticket) {
            Ok(peers) if !peers.is_empty() => {
                join_mode = "direct";
                bootstrap_peers = peers;
            }
            Ok(_) => {
                tracing::warn!("room transport trusted-source bootstrap returned no peer endpoints")
            }
            Err(err) => tracing::warn!("room transport trusted-source bootstrap failed: {}", err),
        }
    }
    join_room_transport_topic_blocking(runtime, topic, join_mode)?;
    if !bootstrap_peers.is_empty() {
        if let Err(err) =
            join_room_transport_bootstrap_peers_blocking(runtime, topic, &bootstrap_peers)
        {
            tracing::warn!(
                "room transport trusted-source topic peer join failed: {}",
                err
            );
        }
    }
    Ok(())
}

fn drain_room_transport_inbound_blocking(
    data_dir: &std::path::Path,
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<(usize, usize)> {
    let mut imported = 0usize;
    let mut dropped = 0usize;
    for _ in 0..ROOM_TRANSPORT_RECV_DRAIN_PAGES {
        let batch = recv_room_transport_messages_blocking(runtime, topic)?;
        for message in batch.messages {
            match verify_and_decode_room_message_blocking(runtime, &message) {
                Some(envelope) => {
                    match crate::room_service::ingest_room_object_envelope(data_dir, &envelope) {
                        Ok(Some(_)) => imported += 1,
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(
                                event_id = %envelope.event_id,
                                sender_member_did = %envelope.sender_member_did,
                                "room transport rejected inbound object: {err}"
                            );
                            dropped += 1;
                        }
                    }
                }
                None => dropped += 1,
            }
        }
        if batch.scanned == 0 || batch.scanned < ROOM_TRANSPORT_RECV_LIMIT as usize {
            break;
        }
    }
    Ok((imported, dropped))
}

fn attach_room_runtime_blocking(
    data_dir: &std::path::Path,
) -> anyhow::Result<AttachedRoomRuntimeBlocking> {
    let coords = load_runtime_coords(data_dir)
        .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not running"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let client_token = attach_client_token_blocking(&client, &coords)
        .ok_or_else(|| anyhow::anyhow!("failed to attach to local ElastOS service"))?;
    let peer_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://peer/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire Carrier peer capability"))?;
    let (room_signing_key, did) = room_transport_identity(data_dir)?;
    Ok(AttachedRoomRuntimeBlocking {
        client,
        api_url: coords.api_url,
        client_token,
        peer_cap,
        did,
        room_signing_key,
    })
}

fn room_transport_identity(
    data_dir: &std::path::Path,
) -> anyhow::Result<(ed25519_dalek::SigningKey, String)> {
    let identity_data_dir = room_transport_identity_data_dir(data_dir);
    let (signing_key, did) = elastos_identity::load_or_create_did(&identity_data_dir)?;
    if let Some(expected_did) = load_existing_gateway_runtime_did(data_dir) {
        if expected_did != did {
            anyhow::bail!(
                "room transport identity mismatch: expected {}, got {} from {}",
                expected_did,
                did,
                identity_data_dir.display()
            );
        }
    }
    Ok((signing_key, did))
}

fn room_transport_identity_data_dir(data_dir: &std::path::Path) -> PathBuf {
    std::env::var_os(HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR_ENV)
        .and_then(|value| {
            let value = value.into_string().ok()?;
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(PathBuf::from(value))
            }
        })
        .unwrap_or_else(|| data_dir.to_path_buf())
}

fn peer_provider_request_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .post(format!("{api}/api/provider/peer/{op}"))
        .header(AUTHORIZATION, format!("Bearer {client_token}"))
        .header("X-Capability-Token", peer_cap)
        .json(&body)
        .send()?;
    let body: serde_json::Value = response.json()?;
    if body.get("status").and_then(|status| status.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("unknown Carrier provider error")
        );
    }
    Ok(body)
}

fn join_room_transport_topic_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
    mode: &str,
) -> anyhow::Result<()> {
    match peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_join",
        serde_json::json!({ "topic": topic, "mode": mode }),
    ) {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("already joined") => Ok(()),
        Err(err) => Err(err),
    }
}

fn remember_room_transport_bootstrap_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    ticket: &str,
) -> anyhow::Result<Vec<String>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "remember_peer",
        serde_json::json!({ "ticket": ticket }),
    )?;
    let mut peers = body
        .get("data")
        .map(|data| {
            ["added", "connected"]
                .into_iter()
                .flat_map(|key| {
                    data.get(key)
                        .and_then(|value| value.as_array())
                        .into_iter()
                        .flatten()
                })
                .filter_map(|value| value.as_str().map(str::trim))
                .filter(|peer| !peer.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    peers.sort();
    peers.dedup();
    Ok(peers)
}

fn join_room_transport_bootstrap_peers_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
    peers: &[String],
) -> anyhow::Result<()> {
    if peers.is_empty() {
        return Ok(());
    }
    let _ = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_join_peers",
        serde_json::json!({ "topic": topic, "peers": peers }),
    )?;
    Ok(())
}

fn room_transport_bootstrap_ticket(data_dir: &std::path::Path) -> Option<String> {
    let config = crate::sources::load_trusted_sources(data_dir).ok()?;
    let source = config.default_source()?;
    if let Some(ticket) = live_gateway_room_transport_bootstrap_ticket(source) {
        return Some(ticket);
    }
    let ticket = source.connect_ticket.trim();
    if ticket.is_empty() {
        None
    } else {
        Some(ticket.to_string())
    }
}

fn live_gateway_room_transport_bootstrap_ticket(
    source: &crate::sources::TrustedSource,
) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    for gateway in crate::sources::normalize_gateways(&source.gateways) {
        let url = format!("{gateway}/.well-known/elastos/carrier-bootstrap.json");
        let Ok(response) = client.get(url).send() else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(document) = response.json::<CarrierBootstrapDocument>() else {
            continue;
        };
        let ticket = document.ticket.trim();
        let ticket = if ticket.is_empty() {
            document.connect_ticket.trim()
        } else {
            ticket
        };
        if !ticket.is_empty() {
            return Some(ticket.to_owned());
        }
    }
    None
}

fn list_room_transport_peers_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<Vec<String>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "list_topic_peers",
        serde_json::json!({ "topic": topic }),
    )?;
    Ok(body
        .get("data")
        .and_then(|data| data.get("peers"))
        .and_then(|value| value.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn recv_room_transport_messages_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<RoomTransportRecvBatch> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": topic,
            "limit": ROOM_TRANSPORT_RECV_LIMIT,
            "consumer_id": ROOM_SYNC_CONSUMER_ID,
            "skip_sender_id": runtime.did,
        }),
    )?;
    let data = body.get("data").unwrap_or(&serde_json::Value::Null);
    let messages: Vec<GatewayGossipMessage> = data
        .get("messages")
        .and_then(|value| value.as_array())
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| {
                    serde_json::from_value::<GatewayGossipMessage>(message.clone()).ok()
                })
                .collect()
        })
        .unwrap_or_default();
    let scanned = data
        .get("scanned")
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(messages.len());
    Ok(RoomTransportRecvBatch { messages, scanned })
}

fn room_envelope_gossip_message(
    runtime: &AttachedRoomRuntimeBlocking,
    envelope: &crate::room_service::RoomObjectEnvelope,
) -> anyhow::Result<crate::carrier::GossipMessage> {
    if envelope.sender_member_did != runtime.did {
        anyhow::bail!(
            "conversation object signer {} does not match local ElastOS identity {}",
            envelope.sender_member_did,
            runtime.did
        );
    }
    let message = serde_json::to_string(envelope)?;
    let signature = sign_room_message_blocking(
        runtime,
        &envelope.sender_member_did,
        envelope.created_at,
        &message,
    )?;
    Ok(crate::carrier::GossipMessage {
        sender_id: envelope.sender_member_did.clone(),
        sender_nick: envelope.sender.clone(),
        content: message,
        ts: envelope.created_at,
        nonce: envelope.created_at,
        signature: Some(signature),
        sender_session_id: None,
    })
}

fn gateway_gossip_message_from_carrier(
    message: crate::carrier::GossipMessage,
) -> GatewayGossipMessage {
    GatewayGossipMessage {
        sender_id: message.sender_id,
        content: message.content,
        ts: message.ts,
        signature: message.signature,
    }
}

fn push_room_transport_bootstrap_blocking(
    data_dir: &std::path::Path,
    topic: &str,
    message: &crate::carrier::GossipMessage,
) -> anyhow::Result<bool> {
    let Some(ticket) = room_transport_bootstrap_ticket(data_dir) else {
        return Ok(false);
    };
    let endpoints = crate::carrier::decode_ticket_endpoints(&ticket);
    if endpoints.is_empty() {
        return Ok(false);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut errors = Vec::new();
        for (index, endpoint) in endpoints.into_iter().enumerate() {
            match crate::carrier::CarrierClient::connect_endpoint_addr(
                endpoint,
                ROOM_TRANSPORT_BOOTSTRAP_CARRIER_TIMEOUT_SECS,
            )
            .await
            {
                Ok(client) => match client.push_gossip_message(topic, message).await {
                    Ok(()) => return Ok(true),
                    Err(err) => errors.push(format!("ticket[{index}] push failed: {err}")),
                },
                Err(err) => errors.push(format!("ticket[{index}] connect failed: {err}")),
            }
        }
        tracing::debug!(
            "room transport trusted-source push exhausted endpoints: {}",
            errors.join(" | ")
        );
        Ok(false)
    })
}

fn pull_room_transport_bootstrap_blocking(
    data_dir: &std::path::Path,
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<(usize, usize)> {
    let Some(ticket) = room_transport_bootstrap_ticket(data_dir) else {
        return Ok((0, 0));
    };
    let endpoints = crate::carrier::decode_ticket_endpoints(&ticket);
    if endpoints.is_empty() {
        return Ok((0, 0));
    }
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let pulled = tokio_runtime.block_on(async {
        let mut errors = Vec::new();
        for (index, endpoint) in endpoints.into_iter().enumerate() {
            match crate::carrier::CarrierClient::connect_endpoint_addr(
                endpoint,
                ROOM_TRANSPORT_BOOTSTRAP_CARRIER_TIMEOUT_SECS,
            )
            .await
            {
                Ok(client) => {
                    match client
                        .pull_gossip_messages(
                            topic,
                            ROOM_TRANSPORT_RECV_LIMIT as usize,
                            Some(&runtime.did),
                        )
                        .await
                    {
                        Ok(messages) => return Ok(messages),
                        Err(err) => errors.push(format!("ticket[{index}] pull failed: {err}")),
                    }
                }
                Err(err) => errors.push(format!("ticket[{index}] connect failed: {err}")),
            }
        }
        anyhow::bail!(
            "room transport trusted-source pull exhausted endpoints: {}",
            errors.join(" | ")
        )
    })?;
    if !pulled.is_empty() {
        tracing::info!(
            topic,
            count = pulled.len(),
            skip_sender_id = %runtime.did,
            "room transport trusted-source pull returned messages"
        );
    }

    let mut imported = 0usize;
    let mut dropped = 0usize;
    for message in pulled {
        let message = gateway_gossip_message_from_carrier(message);
        match verify_and_decode_room_message_blocking(runtime, &message) {
            Some(envelope) => {
                match crate::room_service::ingest_room_object_envelope(data_dir, &envelope) {
                    Ok(Some(_)) => imported += 1,
                    Ok(None) => {}
                    Err(err) => {
                        tracing::warn!(
                            event_id = %envelope.event_id,
                            sender_member_did = %envelope.sender_member_did,
                            "room transport rejected trusted-source object: {err}"
                        );
                        dropped += 1;
                    }
                }
            }
            None => dropped += 1,
        }
    }
    Ok((imported, dropped))
}

fn send_room_object_envelope_blocking(
    data_dir: &std::path::Path,
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
    envelope: &crate::room_service::RoomObjectEnvelope,
) -> anyhow::Result<RoomTransportSendOutcome> {
    let gossip_message = room_envelope_gossip_message(runtime, envelope)?;
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": topic,
            "message": gossip_message.content.clone(),
            "sender": gossip_message.sender_nick.clone(),
            "sender_id": gossip_message.sender_id.clone(),
            "ts": gossip_message.ts,
            "nonce": gossip_message.nonce,
            "signature": gossip_message.signature.clone(),
        }),
    )?;
    let pushed_to_bootstrap =
        push_room_transport_bootstrap_blocking(data_dir, topic, &gossip_message)?;
    tracing::info!(
        event_id = %envelope.event_id,
        sender_member_did = %envelope.sender_member_did,
        direct_broadcast = body
            .get("broadcast")
            .and_then(|value| value.as_str())
            .unwrap_or("remote"),
        pushed_to_bootstrap,
        "room transport sent outbound object"
    );
    if body.get("broadcast").and_then(|value| value.as_str()) == Some("local_only")
        && !pushed_to_bootstrap
    {
        return Ok(RoomTransportSendOutcome::LocalOnly);
    }
    Ok(RoomTransportSendOutcome::Delivered)
}

fn verify_and_decode_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    message: &GatewayGossipMessage,
) -> Option<crate::room_service::RoomObjectEnvelope> {
    if message.sender_id.trim().is_empty() || message.ts == 0 {
        tracing::warn!("room transport rejected message: missing sender or timestamp");
        return None;
    }
    let signature = message.signature.as_deref().unwrap_or_default();
    if !verify_room_message_blocking(
        runtime,
        &message.sender_id,
        message.ts,
        &message.content,
        signature,
    ) {
        tracing::warn!(
            sender_id = %message.sender_id,
            ts = message.ts,
            "room transport rejected message: invalid signature"
        );
        return None;
    }
    let envelope: crate::room_service::RoomObjectEnvelope =
        match serde_json::from_str(&message.content) {
            Ok(envelope) => envelope,
            Err(err) => {
                tracing::warn!(
                    sender_id = %message.sender_id,
                    ts = message.ts,
                    "room transport rejected message: invalid envelope JSON: {err}"
                );
                return None;
            }
        };
    if envelope.sender_member_did != message.sender_id || envelope.created_at != message.ts {
        tracing::warn!(
            event_id = %envelope.event_id,
            envelope_sender = %envelope.sender_member_did,
            message_sender = %message.sender_id,
            envelope_created_at = envelope.created_at,
            message_ts = message.ts,
            "room transport rejected message: envelope metadata mismatch"
        );
        return None;
    }
    Some(envelope)
}

fn sign_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
) -> anyhow::Result<String> {
    if sender_id != runtime.did {
        anyhow::bail!(
            "room message signer {} does not match room transport DID {}",
            sender_id,
            runtime.did
        );
    }
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    let payload = hex::decode(payload_hex)?;
    let signature = runtime.room_signing_key.sign(&payload);
    Ok(hex::encode(signature.to_bytes()))
}

fn verify_room_message_blocking(
    _runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
    signature: &str,
) -> bool {
    if sender_id.trim().is_empty() || signature.trim().is_empty() || ts == 0 {
        return false;
    }
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    let Ok(payload) = hex::decode(payload_hex) else {
        return false;
    };
    let Ok(sig_bytes) = hex::decode(signature) else {
        return false;
    };
    let Ok(sig) = ed25519_dalek::Signature::from_slice(&sig_bytes) else {
        return false;
    };
    crate::crypto::decode_did_key(sender_id)
        .and_then(|key| key.verify(&payload, &sig).map_err(Into::into))
        .is_ok()
}

pub(super) async fn room_service_session_leave(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let secure = request_uses_tls(&headers);
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::leave_session_with_transport(&data_dir, &token)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            let mut response = Json(output.object).into_response();
            let clear_room_cookie = match clear_room_session_cookie_header(secure) {
                Ok(value) => value,
                Err(err) => return room_service_error_response(err),
            };
            let clear_browser_cookie = match clear_browser_session_cookie_header(secure) {
                Ok(value) => value,
                Err(err) => return room_service_error_response(err),
            };
            response.headers_mut().append(SET_COOKIE, clear_room_cookie);
            response
                .headers_mut()
                .append(SET_COOKIE, clear_browser_cookie);
            response
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_poll(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomPollBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let transport = room_transport_view(&state, None).await;
    match tokio::task::spawn_blocking(move || {
        crate::room_service::room_poll(&data_dir, &token, body.since)
    })
    .await
    {
        Ok(Ok(mut output)) => {
            output.transport = transport;
            Json(output).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_objects_send(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomSendBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::append_object_with_transport(&data_dir, &token, &body.body)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            Json(output.object).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_upload_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomUploadStartBody>,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::start_attachment_upload(
            &data_dir,
            &token,
            &body.file_name,
            &body.mime_type,
            body.size_bytes,
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_upload_chunk(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let offset = match upload_offset_from_headers(&headers) {
        Ok(offset) => offset,
        Err(err) => return room_service_error_response(err),
    };
    let bytes = body.to_vec();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::append_attachment_upload_chunk(
            &data_dir, &token, &upload_id, offset, &bytes,
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_upload_finish(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::finish_attachment_upload(&data_dir, &token, &upload_id)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            Json(output.object).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_attachment_get(
    State(state): State<GatewayState>,
    Path(attachment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let data_dir = state.data_dir.clone();
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || {
        crate::room_service::read_attachment(&data_dir, &token, &attachment_id)
    })
    .await
    {
        Ok(Ok((attachment, bytes))) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-type",
                HeaderValue::from_str(&attachment.mime_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );
            let disposition = if attachment.is_image || attachment.is_audio || attachment.is_video {
                "inline"
            } else {
                "attachment"
            };
            let content_disposition = format!(
                "{}; filename=\"{}\"",
                disposition,
                attachment.file_name.replace('"', "")
            );
            headers.insert(
                "content-disposition",
                HeaderValue::from_str(&content_disposition)
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            headers.insert("cache-control", HeaderValue::from_static("no-store"));
            (StatusCode::OK, headers, bytes).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(crate) fn request_uses_tls(headers: &HeaderMap) -> bool {
    if headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
    {
        return true;
    }

    if headers
        .get("forwarded")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("proto=https"))
    {
        return true;
    }

    request_host(headers).is_some_and(|host| !request_host_is_local(&host))
}

fn request_host_is_local(host: &str) -> bool {
    let host = host
        .trim()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let host = if host == "::1" {
        host.as_str()
    } else {
        host.split(':').next().unwrap_or(host.as_str())
    };
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
}

pub(crate) fn set_room_session_cookie_header(
    token: &str,
    max_age_secs: u64,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{ROOM_SESSION_COOKIE}={token}; Max-Age={max_age_secs}; Path=/; HttpOnly; SameSite=Lax"
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn set_browser_session_cookie_header(
    token: &str,
    max_age_secs: u64,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{BROWSER_SESSION_COOKIE}={token}; Max-Age={max_age_secs}; Path=/; HttpOnly; SameSite=Lax"
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn clear_room_session_cookie_header(secure: bool) -> anyhow::Result<HeaderValue> {
    let mut value = format!("{ROOM_SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn clear_browser_session_cookie_header(secure: bool) -> anyhow::Result<HeaderValue> {
    let mut value = format!("{BROWSER_SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

fn room_session_token_from_headers(headers: &HeaderMap) -> anyhow::Result<String> {
    if let Ok(token) = bearer_token_from_headers(headers) {
        return Ok(token);
    }
    if let Some(token) = cookie_value_from_headers(headers, ROOM_SESSION_COOKIE) {
        return Ok(token);
    }
    if let Some(token) = cookie_value_from_headers(headers, BROWSER_SESSION_COOKIE) {
        return Ok(token);
    }
    anyhow::bail!(
        "missing room session. Expected Authorization: Bearer <token> or room/browser session cookie"
    )
}

fn chat_room_access_token_from_headers(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<String> {
    if headers.contains_key("x-elastos-home-token") {
        let context = require_home_launch_token_context(data_dir, headers, CHAT_ROOM_CAPSULE_ID)?;
        return Ok(start_chat_room_session(data_dir, &context)?.token);
    }
    room_session_token_from_headers(headers)
}

pub(crate) fn cookie_value_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie_header| {
            cookie_header.split(';').map(str::trim).find_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                if key.trim() == name {
                    Some(value.trim().to_string())
                } else {
                    None
                }
            })
        })
        .filter(|value| !value.is_empty())
}

fn bearer_token_from_headers(headers: &HeaderMap) -> anyhow::Result<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing Authorization header. Expected: Bearer <token>"))
}

fn upload_offset_from_headers(headers: &HeaderMap) -> anyhow::Result<u64> {
    headers
        .get("x-elastos-upload-offset")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("missing x-elastos-upload-offset header"))?
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("invalid x-elastos-upload-offset header"))
}

pub(super) fn room_service_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid or expired session")
        || text.contains("missing room session")
        || text.contains("home launch token")
    {
        StatusCode::UNAUTHORIZED
    } else if text.contains("not an active member")
        || text.contains("not part of this conversation")
        || text.contains("cannot pair")
    {
        StatusCode::FORBIDDEN
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

pub(super) fn room_service_join_error_response(err: tokio::task::JoinError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("group chat task failed: {}", err),
    )
        .into_response()
}
