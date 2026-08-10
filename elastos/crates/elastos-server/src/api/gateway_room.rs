use super::*;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatDirectMessageSendRequest {
    request_id: String,
    conversation_id: String,
    text: String,
}

fn direct_api_error_response(
    error: crate::collaboration_direct_messages::DirectApiError,
) -> Response {
    use crate::collaboration_direct_messages::DirectApiError;
    let status = match error {
        DirectApiError::InvalidRequest => StatusCode::BAD_REQUEST,
        DirectApiError::ForbiddenConversation => StatusCode::FORBIDDEN,
        DirectApiError::IntentConflict => StatusCode::CONFLICT,
        DirectApiError::Authority => StatusCode::UNAUTHORIZED,
        DirectApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(serde_json::json!({"error": error.to_string()})),
    )
        .into_response()
}

struct DirectAuthorityContext {
    context: HomeLaunchTokenContext,
    service: crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    authority: super::gateway_home_system::ConfiguredContactAuthority,
    // The authority actor the validated launch token proved against the
    // session grant — "home" for browser chat windows.
    authority_app: String,
}

fn direct_authority(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<DirectAuthorityContext, crate::collaboration_direct_messages::DirectApiError> {
    use crate::collaboration_direct_messages::DirectApiError;
    let required =
        require_home_launch_token_binding(&state.data_dir, headers, &[CHAT_ROOM_CAPSULE_ID])
            .map_err(|_| DirectApiError::Authority)?;
    let authority_app = required.launch_context.authority_actor.clone();
    let context = required.context;
    let service = state
        .collaboration_discovery_service
        .clone()
        .ok_or(DirectApiError::Internal)?;
    let authority = super::gateway_home_system::load_configured_contact_authority_for_context(
        &state.data_dir,
        &context,
        Some(&service),
    )
    .map_err(|_| DirectApiError::Internal)?
    .ok_or(DirectApiError::Authority)?;
    Ok(DirectAuthorityContext {
        context,
        service,
        authority,
        authority_app,
    })
}

pub(super) async fn chat_direct_conversations(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let DirectAuthorityContext {
        service, authority, ..
    } = match direct_authority(&state, &headers) {
        Ok(authority) => authority,
        Err(error) => return direct_api_error_response(error),
    };
    match service
        .direct_message_service()
        .conversation_summaries(authority.store.as_ref())
    {
        Ok(conversations) => {
            Json(serde_json::json!({"conversations": conversations})).into_response()
        }
        Err(error) => direct_api_error_response(error),
    }
}

pub(super) async fn chat_direct_conversation_messages(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(conversation_id): Path<String>,
) -> Response {
    let DirectAuthorityContext {
        service, authority, ..
    } = match direct_authority(&state, &headers) {
        Ok(authority) => authority,
        Err(error) => return direct_api_error_response(error),
    };
    match service.direct_message_service().message_summaries(
        authority.store.as_ref(),
        &authority.profile,
        &conversation_id,
        now_ts(),
    ) {
        Ok(messages) => {
            // Reading the conversation is what resolves its message
            // notification; the next incoming message resurfaces it.
            let _ = crate::notifications::mark_acted_for_action(
                &state.data_dir,
                &crate::notifications::direct_message_notification_action_id(&conversation_id),
            );
            Json(serde_json::json!({
                "conversation_id": conversation_id,
                "messages": messages,
            }))
            .into_response()
        }
        Err(error) => direct_api_error_response(error),
    }
}

pub(super) async fn chat_direct_message_send(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    request: Result<Json<ChatDirectMessageSendRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let DirectAuthorityContext {
        context,
        service,
        authority,
        authority_app,
    } = match direct_authority(&state, &headers) {
        Ok(authority) => authority,
        Err(error) => return direct_api_error_response(error),
    };
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            return direct_api_error_response(
                crate::collaboration_direct_messages::DirectApiError::InvalidRequest,
            )
        }
    };
    let now = now_ts();
    match service
        .direct_message_service()
        .send_text_authorized(
            crate::collaboration_direct_messages::DirectSendAuthority {
                contact_store: authority.store.clone(),
                profile: authority.profile.clone(),
                session_id: &context.session_id,
                proof_binding_id: context.proof_binding_id.as_deref(),
                grant_id: &context.grant_id,
                authority_app: &authority_app,
            },
            crate::collaboration_direct_messages::DirectSendIntent {
                request_id: &request.request_id,
                conversation_id: &request.conversation_id,
                text: &request.text,
                now,
            },
        )
        .await
    {
        Ok(crate::collaboration_direct_messages::DirectDeliveryStatus::ReceiptSettled) => {
            Json(serde_json::json!({"status":"receipt_settled"})).into_response()
        }
        Ok(crate::collaboration_direct_messages::DirectDeliveryStatus::Pending) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"status":"pending"})),
        )
            .into_response(),
        Err(error) => direct_api_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoomPollBody {
    #[serde(default)]
    since: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RoomSendBody {
    request_id: String,
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
    contact_id: String,
    #[serde(default)]
    role: Option<crate::room_service::RoomRole>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomMemberRemoveBody {
    member_profile_did: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomInviteRevokeBody {
    invite_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteCreateBody {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteJoinBody {
    #[serde(flatten)]
    _body: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatRoomJoinInviteAcceptanceBody {
    acceptance: crate::room_service::SignedRoomAcceptEnvelope,
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

fn decode_guarded_room_body<T>(body: serde_json::Value) -> Result<T, (StatusCode, String)>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(body).map_err(|err| (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()))
}

pub(super) async fn room_service_summary(State(state): State<GatewayState>) -> Response {
    let data_dir = state.data_dir.clone();
    let summary_result =
        tokio::task::spawn_blocking(move || load_room_summary_with_identity(&data_dir)).await;
    match summary_result {
        Ok(Ok(mut summary)) => {
            summary.transport = room_transport_view(&state);
            Json(GatewayRoomSummary::from(summary)).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn chat_room_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if state.collaboration_chat_product_port.is_none() {
        return room_service_summary(State(state)).await;
    }
    if let Err(err) =
        require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID)
    {
        return room_service_error_response(err);
    }
    Json(serde_json::json!({
        "room_slug": crate::room_service::room_slug(),
        "pending_count": 0,
        "active_session_count": 0,
        "browser_access_allowed": false,
        "browser_access_block_reason": "Configured collaboration Chat is available only through its signed Home projection.",
        "transport": room_transport_view(&state),
    }))
    .into_response()
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
            "Collaboration bootstrap unavailable",
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
                format!("Collaboration bootstrap unavailable: {err}"),
            )
                .into_response())
        }
    };
    if body.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("Collaboration bootstrap unavailable");
        return Err((StatusCode::SERVICE_UNAVAILABLE, message.to_string()).into_response());
    }
    carrier_bootstrap_from_body(&body).ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Collaboration bootstrap unavailable: ticket missing",
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
    if let Some(coords) = load_home_runtime_coords(&data_dir) {
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
            "message",
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
    let transport = room_transport_view(&state);
    let port = state.collaboration_chat_product_port.clone();
    let discovery_service = state.collaboration_discovery_service.clone();
    match tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let output = if port.is_some() {
            start_configured_chat_room_session(&data_dir, &context)?
        } else {
            start_chat_room_session(&data_dir, &context)?
        };
        let configured = port.is_some();
        let mut poll = match port {
            Some(port) => port.conversation_poll(&data_dir, &output.token, 0)?,
            None => crate::room_service::room_poll(&data_dir, &output.token, 0)?,
        };
        // The configured shared room refreshes only already-verified signed
        // Profile names here. The plain room keeps its server-stamped
        // home-session and guest names.
        if configured {
            let authority = gateway_home_system::load_configured_contact_authority_for_context(
                &data_dir,
                &context,
                discovery_service.as_ref(),
            )
            .unwrap_or(None);
            let summary = crate::room_service::load_summary(&data_dir).ok();
            let local_identity = room_local_profile_identity(&data_dir, &context);
            let names = room_profile_attribution_names(
                authority.as_ref(),
                summary.as_ref(),
                local_identity
                    .as_ref()
                    .map(|(did, name)| (did.as_str(), name.as_str())),
            );
            apply_profile_attribution_to_room_poll(&mut poll, &names);
        }
        poll.transport = transport;
        Ok((output, poll))
    })
    .await
    {
        Ok(Ok((output, poll))) => {
            let mut response = Json(ChatRoomSessionStartResponse {
                status: "connected".to_string(),
                display_name: output.display_name,
                expires_at: output.expires_at,
                poll: GatewayRoomPollView::from(poll),
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
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
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
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
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
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let body = match decode_guarded_room_body::<ChatRoomAccessPolicyBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let profile = match trusted_chat_room_profile_authority(&state.data_dir, &context) {
        Ok(profile) => profile,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir, &profile)?;
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let body = match decode_guarded_room_body::<ChatRoomMemberInviteBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let profile = match trusted_chat_room_profile_authority(&state.data_dir, &context) {
        Ok(profile) => profile,
        Err(err) => return room_service_error_response(err),
    };
    let Some(discovery_service) = state.collaboration_discovery_service.as_ref() else {
        return room_service_error_response(anyhow::anyhow!(
            "configured collaboration service is unavailable"
        ));
    };
    let authority = match super::gateway_home_system::load_configured_contact_authority_for_context(
        &state.data_dir,
        &context,
        Some(discovery_service),
    ) {
        Ok(Some(authority)) => authority,
        Ok(None) => {
            return room_service_error_response(anyhow::anyhow!(
                "accepted Profile contact is unavailable"
            ))
        }
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let _ = ensure_local_room_owner_or_actor(&data_dir, &profile)?;
        let snapshot = authority.store.snapshot()?;
        let invited_profile_did = snapshot
            .contacts()
            .iter()
            .find(|contact| {
                super::gateway_home_runtime::home_people_contact_id(contact.remote_profile_did())
                    == body.contact_id
            })
            .map(|contact| contact.remote_profile_did().to_string())
            .ok_or_else(|| anyhow::anyhow!("accepted contact is unavailable"))?;
        let output = crate::room_service::invite_room_member(
            &data_dir,
            crate::room_service::RoomInviteInput {
                invited_profile_did,
                role: body.role.unwrap_or(crate::room_service::RoomRole::Member),
            },
            &profile,
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let body = match decode_guarded_room_body::<ChatRoomMemberRemoveBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let profile = match trusted_chat_room_profile_authority(&state.data_dir, &context) {
        Ok(profile) => profile,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir, &profile)?;
        let output = crate::room_service::remove_room_member(
            &data_dir,
            crate::room_service::RoomMemberRemoveInput {
                actor_did,
                member_did: body.member_profile_did,
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let body = match decode_guarded_room_body::<ChatRoomInviteRevokeBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let profile = match trusted_chat_room_profile_authority(&state.data_dir, &context) {
        Ok(profile) => profile,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let actor_did = ensure_local_room_owner_or_actor(&data_dir, &profile)?;
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let _body = match decode_guarded_room_body::<ChatRoomJoinInviteCreateBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return room_service_error_response(err),
        };
    let profile = match trusted_chat_room_profile_authority(&state.data_dir, &context) {
        Ok(profile) => profile,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let _ = ensure_local_room_owner_or_actor(&data_dir, &profile)?;
        crate::room_service::export_room_join_invite(
            &data_dir,
            crate::room_service::RoomJoinInviteInput {
                inviter_profile: profile.signed_envelope().clone(),
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
    Json(_body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    (
        StatusCode::CONFLICT,
        "Room join claims are unavailable through this legacy transport path.",
    )
        .into_response()
}

pub(super) async fn chat_room_join_invite_acceptance(
    State(state): State<GatewayState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let body = match decode_guarded_room_body::<ChatRoomJoinInviteAcceptanceBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
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
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_legacy_room_control_unsupported_response();
    }
    let _body = match decode_guarded_room_body::<ChatRoomJoinInviteJoinBody>(body) {
        Ok(body) => body,
        Err((status, message)) => return (status, message).into_response(),
    };
    let _ = match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID)
    {
        Ok(context) => context,
        Err(err) => return room_service_error_response(err),
    };
    (
        StatusCode::CONFLICT,
        "Room join claims are unavailable through this legacy transport path.",
    )
        .into_response()
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
    let (did, handle) = trusted_local_chat_room_principal(data_dir, context)?;
    let session = crate::room_service::start_local_principal_runtime_session(
        data_dir,
        &did,
        &context.principal_id,
        &handle,
        "ElastOS shell",
    )?;
    Ok(chat_room_session_grant(session))
}

fn start_configured_chat_room_session(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<ChatRoomSessionGrant> {
    let (did, handle) = trusted_configured_chat_profile(data_dir, context)?;
    let session = crate::room_service::start_configured_collaboration_principal_session(
        data_dir,
        &did,
        &context.principal_id,
        &handle,
        "ElastOS shell",
    )?;
    Ok(chat_room_session_grant(session))
}

fn trusted_local_chat_room_principal(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<(String, String)> {
    let profile = trusted_chat_room_profile_authority(data_dir, context)?;
    Ok((
        profile.document().profile_did.clone(),
        profile.document().display_name.clone(),
    ))
}

fn trusted_configured_chat_profile(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<(String, String)> {
    let profile = trusted_chat_room_profile_authority(data_dir, context)?;
    Ok((
        profile.document().profile_did.clone(),
        profile.document().display_name.clone(),
    ))
}

pub(super) fn trusted_chat_room_profile_authority(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument> {
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    crate::collaboration_profile_authority::load_profile_authority(
        data_dir,
        &context.principal_id,
        &localhost_root,
    )?
    .ok_or_else(|| anyhow::anyhow!("signed Profile is unavailable"))
}

fn chat_room_session_grant(
    session: crate::room_service::LocalRuntimeSessionOutput,
) -> ChatRoomSessionGrant {
    ChatRoomSessionGrant {
        max_age_secs: session.expires_at.saturating_sub(now_ts()),
        token: session.token,
        display_name: session.display_name,
        expires_at: session.expires_at,
    }
}

pub(super) fn ensure_local_room_owner_or_actor(
    data_dir: &std::path::Path,
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
) -> anyhow::Result<String> {
    let did = profile.document().profile_did.clone();
    let control = crate::room_service::load_room_control(data_dir)?;
    if control.owner_did.is_none() {
        let _ = crate::room_service::seed_room_owner(
            data_dir,
            profile,
            crate::room_service::RoomOwnerSeedInput {
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

fn room_transport_view(state: &GatewayState) -> crate::room_service::RoomTransportView {
    state
        .collaboration_chat_product_port
        .as_ref()
        .map(|port| port.conversation_transport_view())
        .unwrap_or_else(|| crate::room_service::RoomTransportView {
            configured: false,
            available: false,
            status: Some("Collaboration is isolated on this Runtime.".to_string()),
        })
}

/// Profile DID → signed Profile display name, for shared-room attribution.
///
/// Sources, in authority order: the local principal's own signed Profile,
/// accepted contacts (head-superseded names), removed contacts (history keeps
/// its signed name after a relationship ends), and room membership profile
/// cards (the signed-profile projection recorded at admission — the source a
/// guest session can verify against). Presence heartbeats are deliberately
/// not a name source: they prove liveness, not identity.
pub(super) fn room_profile_attribution_names(
    contact_authority: Option<&gateway_home_system::ConfiguredContactAuthority>,
    room_summary: Option<&crate::room_service::RoomSummary>,
    local_identity: Option<(&str, &str)>,
) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    if let Some(summary) = room_summary {
        for member in &summary.room_control.members {
            if let Some(card) = &member.profile_card {
                names.insert(member.member_did.clone(), card.display_name.clone());
            }
        }
    }
    if let Some(authority) = contact_authority {
        if let Ok(snapshot) = authority.store.snapshot() {
            for removed in snapshot.removed() {
                names.insert(
                    removed.remote_profile_did().to_string(),
                    removed.display_name().to_string(),
                );
            }
            for contact in snapshot.contacts() {
                names.insert(
                    contact.remote_profile_did().to_string(),
                    contact.remote_display_name().to_string(),
                );
            }
        }
        names.insert(
            authority.profile.document().profile_did.clone(),
            authority.profile.document().display_name.clone(),
        );
    }
    // The polling principal's own signed Profile names its session identity
    // even before any contact store exists — a fresh configured home still
    // names its owner.
    if let Some((member_did, display_name)) = local_identity {
        names.insert(member_did.to_string(), display_name.to_string());
    }
    names
}

/// The polling principal's session member DID and signed Profile display
/// name, when both resolve. This is the launch-context half of the name map;
/// it never invents a name.
fn room_local_profile_identity(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> Option<(String, String)> {
    let (member_did, _) = trusted_configured_chat_profile(data_dir, context).ok()?;
    let card = gateway_home_system::home_room_profile_card_projection(data_dir, context).ok()??;
    Some((member_did, card.display_name))
}

/// Binds configured shared-room attribution to signed Profile identity.
/// A visible row must already carry a verified Profile+endpoint binding from
/// the room projection; this helper may refresh the display name from signed
/// Profile sources, but it never upgrades an unverified row. The plain room
/// keeps its server-stamped home-session and guest names.
pub(super) fn apply_profile_attribution_to_room_poll(
    poll: &mut crate::room_service::RoomPollView,
    names: &std::collections::HashMap<String, String>,
) {
    for object in &mut poll.objects {
        if object.sender_profile_verified != Some(true) {
            continue;
        }
        let Some(did) = object
            .sender_member_did
            .as_deref()
            .map(str::trim)
            .filter(|did| !did.is_empty())
        else {
            continue;
        };
        if let Some(display_name) = names.get(did) {
            object.sender = display_name.clone();
        }
    }
    apply_profile_attribution_to_participants(&mut poll.participants, names);
}

pub(super) fn apply_profile_attribution_to_participants(
    participants: &mut [crate::room_service::ParticipantView],
    names: &std::collections::HashMap<String, String>,
) {
    for participant in participants {
        if participant.profile_verified != Some(true) {
            continue;
        }
        let Some(did) = participant
            .member_did
            .as_deref()
            .map(str::trim)
            .filter(|did| !did.is_empty())
        else {
            continue;
        };
        if let Some(display_name) = names.get(did) {
            participant.display_name = display_name.clone();
        }
    }
}

fn validate_room_send_request_id(request_id: &str) -> anyhow::Result<()> {
    if request_id.is_empty()
        || request_id.len() > 160
        || request_id.trim() != request_id
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        anyhow::bail!("Chat request_id must be a canonical bounded ASCII identifier");
    }
    Ok(())
}

fn configured_attachment_unsupported_response() -> Response {
    (
        StatusCode::CONFLICT,
        "Attachments are not supported in configured collaboration Chat.",
    )
        .into_response()
}

fn configured_legacy_room_control_unsupported_response() -> Response {
    (
        StatusCode::CONFLICT,
        "Legacy room controls are unavailable in configured collaboration Chat.",
    )
        .into_response()
}

pub(super) async fn room_service_session_leave(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let secure = request_uses_tls(&headers);
    let data_dir = state.data_dir.clone();
    if state.collaboration_chat_product_port.is_some() {
        let context =
            match require_home_launch_token_context(&data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
                Ok(context) => context,
                Err(err) => return room_service_error_response(err),
            };
        let (did, _) = match trusted_configured_chat_profile(&data_dir, &context) {
            Ok(principal) => principal,
            Err(err) => return room_service_error_response(err),
        };
        let principal_id = context.principal_id;
        let result = tokio::task::spawn_blocking(move || {
            crate::room_service::leave_configured_collaboration_principal_session(
                &data_dir,
                &did,
                &principal_id,
            )
        })
        .await;
        return match result {
            Ok(Ok(_)) => {
                let mut response =
                    Json(serde_json::json!({"status": "disconnected"})).into_response();
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
        };
    }
    let token = match chat_room_access_token_from_headers(&data_dir, &headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    match tokio::task::spawn_blocking(move || crate::room_service::leave_session(&data_dir, &token))
        .await
    {
        Ok(Ok(object)) => {
            let mut response = Json(object).into_response();
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
    let transport = room_transport_view(&state);
    let port = state.collaboration_chat_product_port.clone();
    let discovery_service = state.collaboration_discovery_service.clone();
    let mut launch_context = None;
    let token = match port.as_ref() {
        Some(_) => {
            let context = match require_home_launch_token_context(
                &data_dir,
                &headers,
                CHAT_ROOM_CAPSULE_ID,
            ) {
                Ok(context) => context,
                Err(err) => return room_service_error_response(err),
            };
            launch_context = Some(context.clone());
            let (did, _) = match trusted_configured_chat_profile(&data_dir, &context) {
                Ok(principal) => principal,
                Err(err) => return room_service_error_response(err),
            };
            let session =
                match crate::room_service::resolve_configured_collaboration_principal_session(
                    &data_dir,
                    &did,
                    &context.principal_id,
                ) {
                    Ok(session) => session,
                    Err(err) => return room_service_error_response(err),
                };
            match optional_configured_room_session_token_from_headers(&headers) {
                Ok(Some(provided)) if provided != session.token => {
                    return room_service_error_response(anyhow::anyhow!(
                        "invalid or expired session for configured Chat"
                    ));
                }
                Ok(_) => session.token,
                Err(err) => return room_service_error_response(err),
            }
        }
        None => match chat_room_access_token_from_headers(&data_dir, &headers) {
            Ok(token) => token,
            Err(err) => return room_service_error_response(err),
        },
    };
    match tokio::task::spawn_blocking(move || {
        let mut poll = match port {
            Some(port) => port.conversation_poll(&data_dir, &token, body.since)?,
            None => crate::room_service::room_poll(&data_dir, &token, body.since)?,
        };
        // The configured shared room refreshes only already-verified signed
        // Profile names here: contact authority plus room membership cards
        // may refresh the display name, but they never upgrade an unverified
        // row. The plain room keeps its server-stamped home-session and
        // guest names.
        if let Some(context) = launch_context.as_ref() {
            let authority = gateway_home_system::load_configured_contact_authority_for_context(
                &data_dir,
                context,
                discovery_service.as_ref(),
            )
            .unwrap_or(None);
            let summary = crate::room_service::load_summary(&data_dir).ok();
            let local_identity = room_local_profile_identity(&data_dir, context);
            let names = room_profile_attribution_names(
                authority.as_ref(),
                summary.as_ref(),
                local_identity
                    .as_ref()
                    .map(|(did, name)| (did.as_str(), name.as_str())),
            );
            apply_profile_attribution_to_room_poll(&mut poll, &names);
        }
        Ok::<_, anyhow::Error>(poll)
    })
    .await
    {
        Ok(Ok(mut output)) => {
            output.transport = transport;
            Json(GatewayRoomPollView::from(output)).into_response()
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
    if let Err(err) = validate_room_send_request_id(&body.request_id) {
        return room_service_error_response(err);
    }
    let data_dir = state.data_dir.clone();
    let port = state.collaboration_chat_product_port.clone();
    let context = match port.as_ref() {
        Some(_) => {
            match require_home_launch_token_context(&data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
                Ok(context) => Some(context),
                Err(err) => return room_service_error_response(err),
            }
        }
        None => None,
    };
    let profile = match context.as_ref() {
        Some(context) => {
            match gateway_home_system::load_profile_authority_for_context(&data_dir, context) {
                Ok(Some(profile)) => Some(profile),
                Ok(None) => {
                    return room_service_error_response(anyhow::anyhow!(
                        "Chat requires a signed Profile"
                    ))
                }
                Err(err) => return room_service_error_response(err),
            }
        }
        None => None,
    };
    let token = match context.as_ref() {
        Some(context) => match start_configured_chat_room_session(&data_dir, context) {
            Ok(session) => session.token,
            Err(err) => return room_service_error_response(err),
        },
        None => match chat_room_access_token_from_headers(&data_dir, &headers) {
            Ok(token) => token,
            Err(err) => return room_service_error_response(err),
        },
    };
    let now = now_ts();
    match tokio::task::spawn_blocking(move || match (port, context, profile) {
        (Some(port), Some(context), Some(profile)) => {
            let operation = crate::collaboration_product::chat_message_request_binding(
                &body.request_id,
                &context.principal_id,
                &body.body,
                &profile,
            )?;
            let prepared = port.prepare_message(operation, &body.body, &profile, now)?;
            port.project_prepared_message(&data_dir, &prepared, Some(&token))
        }
        (None, None, None) => crate::room_service::append_object(&data_dir, &token, &body.body),
        _ => anyhow::bail!("Chat collaboration authority is inconsistent"),
    })
    .await
    {
        Ok(Ok(object)) => Json(object).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_upload_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomUploadStartBody>,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_attachment_unsupported_response();
    }
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
    if state.collaboration_chat_product_port.is_some() {
        return configured_attachment_unsupported_response();
    }
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
    if state.collaboration_chat_product_port.is_some() {
        return configured_attachment_unsupported_response();
    }
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
        Ok(Ok(output)) => Json(output.object).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn room_service_attachment_get(
    State(state): State<GatewayState>,
    Path(attachment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if state.collaboration_chat_product_port.is_some() {
        return configured_attachment_unsupported_response();
    }
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
    effective_gateway_origin(headers)
        .map(|origin| origin.secure())
        .unwrap_or(false)
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

fn optional_configured_room_session_token_from_headers(
    headers: &HeaderMap,
) -> anyhow::Result<Option<String>> {
    if headers.contains_key(AUTHORIZATION) {
        return bearer_token_from_headers(headers)
            .map(Some)
            .map_err(|_| anyhow::anyhow!("invalid or expired session for configured Chat"));
    }
    Ok(cookie_value_from_headers(headers, ROOM_SESSION_COOKIE))
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
    let profile_required = gateway_home_system::profile_required_message(&err);
    let text = profile_required
        .map(str::to_string)
        .unwrap_or_else(|| err.to_string());
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid or expired session")
        || text.contains("missing room session")
        || text.contains("home launch token")
    {
        StatusCode::UNAUTHORIZED
    } else if profile_required.is_some() {
        StatusCode::CONFLICT
    } else if text.contains("not an active member")
        || text.contains("not part of this conversation")
        || text.contains("cannot pair")
    {
        StatusCode::FORBIDDEN
    } else if text.contains("must not be empty")
        || text.contains("characters or fewer")
        || text.contains("exceeds")
        || text.contains("request_id")
        || text.contains("request binding")
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
