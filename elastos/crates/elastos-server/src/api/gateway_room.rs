use super::*;

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
            summary.transport = room_transport_view(&state, None).await;
            Json(GatewayRoomSummary::from(summary)).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_summary(State(state): State<GatewayState>) -> Response {
    room_service_summary(State(state)).await
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
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "browser access request not found").into_response(),
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
            crate::room_service::deny_request(&data_dir, &request_id, "Denied from Chat Room.")?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        Ok::<_, anyhow::Error>(outcome)
    })
    .await
    {
        Ok(Ok(Some(output))) => Json(output).into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "browser access request not found").into_response(),
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
}

fn start_chat_room_session(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<ChatRoomSessionGrant> {
    let identity = load_gateway_identity_summary_for_context(data_dir, context);
    let did = identity
        .device_did
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    let handle = identity
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("proof-bound passkey handle unavailable"))?;
    let session = crate::room_service::start_local_principal_runtime_session(
        data_dir,
        did,
        &context.principal_id,
        handle,
        "ElastOS shell",
    )?;
    Ok(ChatRoomSessionGrant {
        max_age_secs: session.expires_at.saturating_sub(now_ts()),
        token: session.token,
        display_name: session.display_name,
        expires_at: session.expires_at,
    })
}

fn ensure_local_room_owner_or_actor(data_dir: &std::path::Path) -> anyhow::Result<String> {
    let identity = load_gateway_identity_summary(data_dir);
    let did = identity
        .device_did
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    let control = crate::room_service::load_room_control(data_dir)?;
    if control.owner_did.is_none() {
        let _ = crate::room_service::seed_room_owner(
            data_dir,
            crate::room_service::RoomOwnerSeedInput {
                owner_did: did.clone(),
                title: "Chat Room".to_string(),
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

fn did_provider_request_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    did_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let body: serde_json::Value = client
        .post(format!("{}/api/provider/did/{}", api, op))
        .header("Authorization", format!("Bearer {}", client_token))
        .header("X-Capability-Token", did_cap)
        .json(&body)
        .send()?
        .json()?;
    if body.get("status").and_then(|s| s.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown did-provider error")
        );
    }
    Ok(body)
}

#[derive(Debug, Deserialize)]
struct GatewayGossipMessage {
    sender_id: String,
    content: String,
    ts: u64,
    #[serde(default)]
    signature: Option<String>,
}

struct AttachedRoomRuntimeBlocking {
    client: reqwest::blocking::Client,
    api_url: String,
    client_token: String,
    did_cap: String,
    peer_cap: String,
    did: String,
}

async fn room_transport_view(
    state: &GatewayState,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> crate::room_service::RoomTransportView {
    let data_dir = state.data_dir.clone();
    let topic = room_transport_topic(crate::room_service::room_slug());
    match tokio::task::spawn_blocking(move || sync_room_transport_blocking(&data_dir, outbound))
        .await
    {
        Ok(Ok(view)) => view,
        Ok(Err(err)) => room_transport_error_view(&topic, &err.to_string()),
        Err(err) => {
            room_transport_error_view(&topic, &format!("room transport task failed: {err}"))
        }
    }
}

fn room_transport_topic(room_slug: &str) -> String {
    format!("__elastos_internal/room-sync-v1/{room_slug}")
}

fn room_transport_error_view(topic: &str, detail: &str) -> crate::room_service::RoomTransportView {
    crate::room_service::RoomTransportView {
        available: false,
        connected_peer_count: 0,
        topic: Some(topic.to_string()),
        status: Some(format!("Carrier room sync unavailable: {detail}")),
    }
}

fn sync_room_transport_blocking(
    data_dir: &std::path::Path,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> anyhow::Result<crate::room_service::RoomTransportView> {
    let topic = room_transport_topic(crate::room_service::room_slug());
    let runtime = attach_room_runtime_blocking(data_dir)?;
    let access = crate::room_service::local_runtime_access(data_dir, Some(&runtime.did))?;
    let Some(_member_role) = access.member_role else {
        return Ok(crate::room_service::RoomTransportView {
            available: false,
            connected_peer_count: 0,
            topic: Some(topic),
            status: Some(access.block_reason.unwrap_or_else(|| {
                "Carrier room sync inactive: this runtime is not an active room member.".to_string()
            })),
        });
    };

    let outbound_envelopes =
        crate::room_service::room_transport_backlog(data_dir, &runtime.did, outbound.as_ref())?;
    join_room_transport_topic_blocking(&runtime, &topic)?;
    for envelope in &outbound_envelopes {
        send_room_object_envelope_blocking(&runtime, &topic, envelope)?;
    }

    let mut imported = 0usize;
    let mut dropped = 0usize;
    for message in recv_room_transport_messages_blocking(&runtime, &topic)? {
        match verify_and_decode_room_message_blocking(&runtime, &message) {
            Some(envelope) => {
                match crate::room_service::ingest_room_object_envelope(data_dir, &envelope) {
                    Ok(Some(_)) => imported += 1,
                    Ok(None) => {}
                    Err(_) => dropped += 1,
                }
            }
            None => dropped += 1,
        }
    }

    let connected_peer_count = list_room_transport_peers_blocking(&runtime, &topic)?.len();
    let mut status = if connected_peer_count > 0 {
        format!(
            "Carrier room sync connected to {} runtime{}.",
            connected_peer_count,
            if connected_peer_count == 1 { "" } else { "s" }
        )
    } else {
        "Carrier room sync ready; waiting for another room member runtime.".to_string()
    };
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
        connected_peer_count,
        topic: Some(topic),
        status: Some(status),
    })
}

fn attach_room_runtime_blocking(
    data_dir: &std::path::Path,
) -> anyhow::Result<AttachedRoomRuntimeBlocking> {
    let coords = load_runtime_coords(data_dir)
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let client_token = attach_client_token_blocking(&client, &coords)
        .ok_or_else(|| anyhow::anyhow!("failed to attach to local runtime"))?;
    let did_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://did/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire DID capability"))?;
    let peer_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://peer/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire Carrier peer capability"))?;
    let did = did_provider_request_blocking(
        &client,
        &coords.api_url,
        &client_token,
        &did_cap,
        "get_did",
        serde_json::json!({}),
    )?
    .get("data")
    .and_then(|d| d.get("did"))
    .and_then(|value| value.as_str())
    .map(str::trim)
    .filter(|did| !did.is_empty())
    .map(ToOwned::to_owned)
    .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    Ok(AttachedRoomRuntimeBlocking {
        client,
        api_url: coords.api_url,
        client_token,
        did_cap,
        peer_cap,
        did,
    })
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
) -> anyhow::Result<()> {
    match peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_join",
        serde_json::json!({ "topic": topic }),
    ) {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("already joined") => Ok(()),
        Err(err) => Err(err),
    }
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
) -> anyhow::Result<Vec<GatewayGossipMessage>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": topic,
            "limit": 64,
            "consumer_id": ROOM_SYNC_CONSUMER_ID,
            "skip_sender_id": runtime.did,
        }),
    )?;
    Ok(body
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(|value| value.as_array())
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| {
                    serde_json::from_value::<GatewayGossipMessage>(message.clone()).ok()
                })
                .collect()
        })
        .unwrap_or_default())
}

fn send_room_object_envelope_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
    envelope: &crate::room_service::RoomObjectEnvelope,
) -> anyhow::Result<()> {
    if envelope.sender_member_did != runtime.did {
        anyhow::bail!(
            "room object signer {} does not match local runtime DID {}",
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
    let _ = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": topic,
            "message": message,
            "sender": envelope.sender,
            "sender_id": envelope.sender_member_did,
            "ts": envelope.created_at,
            "signature": signature,
        }),
    )?;
    Ok(())
}

fn verify_and_decode_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    message: &GatewayGossipMessage,
) -> Option<crate::room_service::RoomObjectEnvelope> {
    if message.sender_id.trim().is_empty() || message.ts == 0 {
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
        return None;
    }
    let envelope: crate::room_service::RoomObjectEnvelope =
        serde_json::from_str(&message.content).ok()?;
    if envelope.sender_member_did != message.sender_id || envelope.created_at != message.ts {
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
    did_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.did_cap,
        "sign_chat_message",
        serde_json::json!({
            "sender_id": sender_id,
            "ts": ts,
            "content": content,
        }),
    )?
    .get("data")
    .and_then(|data| data.get("signature"))
    .and_then(|value| value.as_str())
    .map(ToOwned::to_owned)
    .ok_or_else(|| anyhow::anyhow!("did-provider sign response missing signature"))
}

fn verify_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
    signature: &str,
) -> bool {
    if sender_id.trim().is_empty() || signature.trim().is_empty() || ts == 0 {
        return false;
    }
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    did_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.did_cap,
        "verify",
        serde_json::json!({
            "did": sender_id,
            "data": payload_hex,
            "signature": signature,
        }),
    )
    .ok()
    .and_then(|body| {
        body.get("data")
            .and_then(|data| data.get("valid"))
            .and_then(|value| value.as_bool())
    })
    .unwrap_or(false)
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
    match tokio::task::spawn_blocking(move || crate::room_service::leave_session(&data_dir, &token))
        .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, None).await;
            let mut response = Json(output).into_response();
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

fn room_service_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid or expired session")
        || text.contains("missing room session")
        || text.contains("home launch token")
    {
        StatusCode::UNAUTHORIZED
    } else if text.contains("not an active member") || text.contains("cannot pair") {
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

fn room_service_join_error_response(err: tokio::task::JoinError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("group chat task failed: {}", err),
    )
        .into_response()
}
