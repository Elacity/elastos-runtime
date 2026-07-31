use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context as _;
use elastos_common::CapsuleRole;

use super::*;

const HOME_EVENTS_SCHEMA: &str = "elastos.home.events/v1";
const HOME_EVENTS_DEFAULT_WAIT_MS: u64 = 25_000;
const HOME_EVENTS_MAX_WAIT_MS: u64 = 30_000;
const HOME_EVENTS_POLL_MS: u64 = 1_000;
const HOME_EVENTS_RETRY_MS: u64 = 250;
const HOME_EVENTS_STREAM_KEEPALIVE_SECS: u64 = 15;
const HOME_DESKTOP_OBJECTS_SCHEMA: &str = "elastos.home.desktop-objects/v1";
const HOME_SYSTEM_DESKTOP_OBJECT_SCHEMA: &str = "elastos.home.system-desktop-object/v1";
const HOME_ACTIVE_SHELL_SCHEMA: &str = "elastos.home.active-shell/v1";
const HOME_ACTIVE_SHELL_MAX_BYTES: usize = 4 * 1024;
const HOME_PROFILE_CARD_SCHEMA: &str = "elastos.profile-card/v1";
const HOME_PROFILE_CARD_MAX_BYTES: usize = 4 * 1024;
const HOME_PEOPLE_CONTACTS_SCHEMA: &str = "elastos.people.contacts-state/v1";
const HOME_PEOPLE_CONTACTS_MAX_BYTES: usize = 32 * 1024;
const HOME_PEOPLE_REMOVED_CONTACTS_SCHEMA: &str = "elastos.people.removed-contacts/v1";
const HOME_PEOPLE_REMOVED_CONTACTS_MAX_BYTES: usize = 16 * 1024;
const HOME_PEOPLE_DISCOVERY_SCHEMA: &str = "elastos.people.discovery/v1";
const HOME_PEOPLE_DISCOVERY_STATE_SCHEMA: &str = "elastos.people.discovery-state/v1";
const HOME_PEOPLE_DISCOVERY_PEERS_SCHEMA: &str = "elastos.people.discovery-peers/v1";
const HOME_PEOPLE_DISCOVERY_MAX_BYTES: usize = 64 * 1024;
const HOME_PEOPLE_DISCOVERY_PEERS_MAX_BYTES: usize = 64 * 1024;
const HOME_PEOPLE_DISCOVERY_TOPIC: &str = "__elastos_internal/people-discovery-v1";
const HOME_PEOPLE_DISCOVERY_ENABLED_SECS: u64 = 10 * 60;
const HOME_PEOPLE_DISCOVERY_BOOTSTRAP_INTERVAL_SECS: u64 = 30;
const HOME_PEOPLE_DISCOVERY_PRESENCE_INTERVAL_SECS: u64 = 15;
const HOME_PEOPLE_DISCOVERY_REFRESH_FAST_MS: u64 = 3_000;
const HOME_PEOPLE_DISCOVERY_REFRESH_SEARCH_MS: u64 = 5_000;
const HOME_PEOPLE_DISCOVERY_REFRESH_IDLE_MS: u64 = 30_000;
const HOME_SERVICES_STATE_SCHEMA: &str = "elastos.services.state/v1";
const HOME_SERVICES_STATE_MAX_BYTES: usize = 32 * 1024;
const HOME_SERVICES_REQUESTS_SCHEMA: &str = "elastos.services.requests/v1";
const HOME_SERVICES_REQUESTS_MAX_BYTES: usize = 64 * 1024;
const HOME_SERVICES_REQUESTS_TOPIC: &str = "__elastos_internal/service-requests-v1";
const HOME_SERVICES_REMOTE_EXIT_TICKET_MAX_BYTES: usize = 8192;
const HOME_BROWSER_EXIT_LOCAL_OFFER_ID: &str = "local:provider:browser-exit";
const HOME_BROWSER_EXIT_PEER_SERVICE_URI: &str = "elastos://peer/browser-exit";
const HOME_REMOTE_EXIT_SERVICE_KIND: &str = "remote_exit";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeEventsQuery {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct HomeEventsResponse {
    schema: String,
    cursor: String,
    keepalive: bool,
    retry_after_ms: u64,
    events: Vec<HomeRealtimeEvent>,
}

#[derive(Debug, Serialize)]
struct HomeRealtimeEvent {
    kind: String,
    scope: String,
    at: u64,
}

#[derive(Debug, Serialize)]
struct HomeRealtimeSnapshot {
    principal_id: String,
    notification_signature: Vec<String>,
    wallet_request_signature: Vec<String>,
    capability_request_count: usize,
    desktop_signature: Vec<String>,
    room_signature: String,
    people_signature: Vec<String>,
    services_signature: Vec<String>,
    browser_sessions: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomePeopleRemovedContacts {
    schema: String,
    principal_id: String,
    localhost_root: String,
    #[serde(default)]
    contacts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomePeopleContactsState {
    schema: String,
    principal_id: String,
    localhost_root: String,
    updated_at: u64,
    #[serde(default)]
    contacts: BTreeMap<String, HomePeopleContactRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomePeopleContactRecord {
    contact_id: String,
    peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    added_at: u64,
    updated_at: u64,
    source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomePeopleDiscoveryState {
    schema: String,
    principal_id: String,
    localhost_root: String,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled_until: Option<u64>,
    updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_peer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_bootstrap_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_presence_sent_at: Option<u64>,
    #[serde(default)]
    peers: BTreeMap<String, HomePeopleDiscoveryPeerSummary>,
    #[serde(default)]
    requests: BTreeMap<String, HomePeopleDiscoveryRequestSummary>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HomePeopleDiscoveryPeersConfig {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    peers: Vec<HomePeopleDiscoveryPeerConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HomePeopleDiscoveryPeerConfig {
    #[serde(default)]
    connect_ticket: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomeServicesSelectionState {
    schema: String,
    principal_id: String,
    localhost_root: String,
    updated_at: u64,
    #[serde(default)]
    local_offer_ids: BTreeSet<String>,
    #[serde(default)]
    remote_offer_ids: BTreeSet<String>,
    #[serde(default)]
    remote_offer_requests: BTreeMap<String, HomeServicesRemoteOfferRequestRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomeServicesRemoteOfferRequestRecord {
    request_id: String,
    offer_id: String,
    service_uri: String,
    service_kind: String,
    service_display_name: String,
    target_peer_id: String,
    created_at: u64,
    updated_at: u64,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installed_remote_exit_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomeServicesRequestsState {
    schema: String,
    principal_id: String,
    localhost_root: String,
    updated_at: u64,
    #[serde(default)]
    requests: BTreeMap<String, HomeServiceAccessRequestRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomeServiceAccessRequestRecord {
    request_id: String,
    offer_id: String,
    service_uri: String,
    service_kind: String,
    service_display_name: String,
    requester_peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requester_did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requester_principal_id: Option<String>,
    requester_display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requester_handle: Option<String>,
    created_at: u64,
    updated_at: u64,
    status: String,
}

#[derive(Debug, Clone)]
struct HomeServiceAccessRequestSent {
    request_id: String,
    target_peer_id: String,
    created_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServicesOfferUpdateRequest {
    offer_id: String,
    section: String,
    selected: bool,
}

struct PeoplePeerRuntimeBlocking {
    client: reqwest::blocking::Client,
    api_url: String,
    client_token: String,
    peer_cap: String,
    peer_id: String,
    connect_ticket: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PeopleDiscoveryUpdateRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PeopleDiscoveryRequestCreate {
    peer_id: String,
}

#[derive(Debug, Serialize)]
struct PeopleDiscoveryRequestJoinResponse {
    schema: String,
    status: String,
    invite_id: String,
    member_did: String,
    discovery: HomePeopleDiscoverySummary,
}

pub(super) async fn home_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let wallet_authority =
        require_home_active_shell_wallet_authority(&state.data_dir, &headers).ok();
    let context = wallet_authority
        .as_ref()
        .map(RuntimeWalletAuthority::home_launch_context);

    let (identity, authority, browser_state, appearance, runtime, home_state) =
        if let Some(context) = context.as_ref() {
            let identity = load_gateway_identity_summary_for_context(&state.data_dir, context);
            let data_dir = state.data_dir.clone();
            let (runtime, mut home_state) =
                tokio::join!(home_runtime_summary(&state.data_dir), async move {
                    tokio::task::spawn_blocking(move || home_state(&data_dir))
                        .await
                        .unwrap_or_default()
                });
            if let Err(err) = apply_home_people_contacts_state(
                &state.data_dir,
                context,
                &mut home_state.people,
                &mut home_state.services,
            ) {
                return home_error_response(err);
            }
            if let Err(err) = filter_removed_people_contacts(
                &state.data_dir,
                context,
                &mut home_state.people,
                &mut home_state.services,
            ) {
                return home_error_response(err);
            }
            home_state.people.discovery =
                match home_people_discovery_summary(&state.data_dir, context) {
                    Ok(discovery) => discovery,
                    Err(err) => return home_error_response(err),
                };
            if let Err(err) =
                apply_home_services_selection(&state.data_dir, context, &mut home_state.services)
            {
                return home_error_response(err);
            }
            let browser_state = match home_browser_state(&state.data_dir, context) {
                Ok(state) => state,
                Err(err) => return home_error_response(err),
            };
            (
                identity,
                home_authority_summary(context),
                browser_state,
                match home_appearance_summary(&state.data_dir, context) {
                    Ok(appearance) => appearance,
                    Err(err) => return home_error_response(err),
                },
                runtime,
                home_state,
            )
        } else {
            (
                standard_home_identity_summary(),
                standard_home_authority_summary(),
                standard_home_browser_state(),
                standard_home_appearance_summary(),
                HomeRuntimeSummary::default(),
                HomeState::default(),
            )
        };

    let mut notifications = home_state.notifications;
    let active_shell = match home_active_shell_summary(&state.data_dir, context.as_ref()) {
        Ok(shell) => shell,
        Err(err) => return home_error_response(err),
    };
    let desktop_objects = if let Some(context) = context.as_ref() {
        home_desktop_objects_summary(&state, context).await
    } else {
        standard_home_desktop_objects_summary()
    };
    if let (Some(context), Some(authority)) = (context.as_ref(), wallet_authority.as_ref()) {
        let wallet_approvals = system_wallet_approvals_summary(&state, authority, false).await;
        append_wallet_approval_notifications(
            &mut notifications,
            wallet_approvals.approval_requests,
        );
        if let Ok(capability_requests) = runtime_capability_pending_requests(&state.data_dir).await
        {
            append_runtime_capability_notifications(&mut notifications, capability_requests);
        }
        append_home_service_access_notifications(&state.data_dir, context, &mut notifications);
    }

    let capsule_catalog = capsule_catalog_summary(&state.data_dir);
    let targets = home_targets_from_catalog(&capsule_catalog);
    let capsule_interfaces = capsule_interface_registry_summary_with_bindings(
        &state.data_dir,
        state.provider_registry.as_deref(),
    )
    .await;

    Json(HomeSummaryResponse {
        home: HomeRouteInfo {
            route: HOME_ROUTE.to_string(),
            attach_kind: "iframe".to_string(),
        },
        app: HomeCapsuleIdentity {
            id: HOME_CAPSULE_ID.to_string(),
            route: HOME_ROUTE.to_string(),
        },
        identity,
        authority,
        browser_state,
        active_shell,
        appearance,
        runtime,
        site: home_state.site,
        room: home_state.room,
        people: home_state.people,
        services: home_state.services,
        notifications,
        desktop_objects,
        capsule_catalog,
        capsule_interfaces,
        targets,
    })
    .into_response()
}

pub(super) async fn people_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let identity = load_gateway_identity_summary_for_context(&state.data_dir, &context);
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let mut state = home_state(&data_dir);
        apply_home_people_contacts_state(
            &data_dir,
            &context,
            &mut state.people,
            &mut state.services,
        )?;
        filter_removed_people_contacts(
            &data_dir,
            &context,
            &mut state.people,
            &mut state.services,
        )?;
        state.people.discovery = home_people_discovery_summary(&data_dir, &context)?;
        Ok::<_, anyhow::Error>(serde_json::json!({
            "schema": "elastos.people.summary/v1",
            "identity": identity,
            "people": state.people,
        }))
    })
    .await
    {
        Ok(Ok(summary)) => Json(summary).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn people_invite_create(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let issuer_gateway = match chat_room_invite_gateway_origin(&headers) {
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
    let context = context.clone();
    match tokio::task::spawn_blocking(move || {
        ensure_local_principal_room_session(&data_dir, &context)?;
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

pub(super) async fn people_contact_remove(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<PeopleContactRemoveRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let contact_id = body.contact_id.trim().to_string();
    if contact_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing contact id").into_response();
    }
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking({
        let contact_id = contact_id.clone();
        let context = context.clone();
        move || {
            let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
            let local_did = load_gateway_identity_summary(&data_dir).device_did;
            if !home_people_contact_exists(
                &data_dir,
                &context,
                &summary,
                local_did.as_deref(),
                contact_id.as_str(),
            )? {
                anyhow::bail!("people contact not found");
            }
            home_mark_people_contact_removed(&data_dir, &context, &contact_id, now_ts())?;
            Ok::<_, anyhow::Error>(())
        }
    })
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({
            "schema": "elastos.people.contact-remove/v1",
            "contact_id": contact_id,
            "scope": "local_people",
            "message": "Removed contact from People."
        }))
        .into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(super) async fn people_profile_card_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemHandleUpdateRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    match update_profile_card_for_context(&state.data_dir, &context, &req.handle) {
        Ok(identity) => Json(identity).into_response(),
        Err(err) => home_error_response(err),
    }
}

async fn ensure_people_discovery_runtime_available(
    data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    if load_runtime_coords(data_dir).is_some() {
        return Ok(());
    }
    crate::runtime_control::ensure_runtime_for_home(data_dir)
        .await
        .map(|_| ())
}

pub(super) async fn people_discovery_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<PeopleDiscoveryUpdateRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let mut discovery = home_people_discovery_state(&data_dir, &context)?;
        let now = now_ts();
        discovery.enabled = body.enabled;
        discovery.enabled_until = body
            .enabled
            .then_some(now + HOME_PEOPLE_DISCOVERY_ENABLED_SECS);
        discovery.updated_at = now;
        if !discovery.enabled {
            discovery.local_peer_id = None;
            discovery.last_bootstrap_at = None;
            discovery.last_presence_sent_at = None;
        }
        if discovery.enabled {
            let _ = home_people_discovery_sync(&data_dir, &context, &mut discovery);
        }
        home_save_people_discovery_state(&data_dir, &context, &discovery)?;
        let mut summary = home_people_discovery_summary(&data_dir, &context)?;
        home_people_discovery_annotate_refresh(&mut summary, true);
        Ok(summary)
    })
    .await
    {
        Ok(Ok(discovery)) => Json(discovery).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn people_discovery_refresh(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    if let Err(err) = ensure_people_discovery_runtime_available(&data_dir).await {
        return home_error_response(anyhow::anyhow!(
            "managed local runtime could not start: {err}"
        ));
    }
    match tokio::task::spawn_blocking(move || {
        let mut discovery = home_people_discovery_state(&data_dir, &context)?;
        let now = now_ts();
        let before_signature = home_people_discovery_state_signature(&discovery);
        let expired = home_people_discovery_apply_expiry(&mut discovery, now);
        if home_people_discovery_active(&discovery, now) {
            home_people_discovery_sync(&data_dir, &context, &mut discovery)?;
            discovery.updated_at = now;
            home_save_people_discovery_state(&data_dir, &context, &discovery)?;
        } else if expired {
            home_save_people_discovery_state(&data_dir, &context, &discovery)?;
        }
        let after_signature = home_people_discovery_state_signature(&discovery);
        let mut summary = home_people_discovery_summary(&data_dir, &context)?;
        home_people_discovery_annotate_refresh(
            &mut summary,
            expired || before_signature != after_signature,
        );
        Ok(summary)
    })
    .await
    {
        Ok(Ok(discovery)) => Json(discovery).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn people_discovery_request_create(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<PeopleDiscoveryRequestCreate>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    if let Err(err) = ensure_people_discovery_runtime_available(&data_dir).await {
        return home_error_response(anyhow::anyhow!(
            "managed local runtime could not start: {err}"
        ));
    }
    match tokio::task::spawn_blocking(move || {
        let peer_id = body.peer_id.trim();
        if peer_id.is_empty() || peer_id.len() > 256 {
            anyhow::bail!("discovery peer id is invalid");
        }
        let mut discovery = home_people_discovery_state(&data_dir, &context)?;
        let peer = discovery
            .peers
            .get(peer_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("discovery peer is not visible"))?;
        let now = now_ts();
        let request_id = home_people_discovery_request_id(peer_id)?;
        let request = HomePeopleDiscoveryRequestSummary {
            request_id: request_id.clone(),
            peer_id: peer.peer_id,
            did: peer.did,
            display_name: peer.display_name,
            handle: peer.handle,
            created_at: now,
            status: "requested".to_string(),
            invite_id: None,
        };
        if discovery.enabled {
            home_people_discovery_send_request(&data_dir, &context, &mut discovery, &request)
                .context("people discovery delivery failed")?;
        }
        discovery.requests.insert(request_id, request);
        discovery.updated_at = now;
        home_save_people_discovery_state(&data_dir, &context, &discovery)?;
        home_people_discovery_summary(&data_dir, &context)
    })
    .await
    {
        Ok(Ok(discovery)) => Json(discovery).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn people_discovery_request_accept(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    if let Err(err) = ensure_people_discovery_runtime_available(&data_dir).await {
        return home_error_response(anyhow::anyhow!(
            "managed local runtime could not start: {err}"
        ));
    }
    match tokio::task::spawn_blocking(move || {
        let request_id = request_id.trim().to_string();
        if request_id.is_empty() || request_id.len() > 256 {
            anyhow::bail!("discovery request id is invalid");
        }
        let mut discovery = home_people_discovery_state(&data_dir, &context)?;
        let request = discovery
            .requests
            .get(&request_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("discovery request not found"))?;
        if request.status != "incoming"
            && request.status != "accepted_sent"
            && request.status != "joined"
        {
            anyhow::bail!("discovery request is not waiting for acceptance");
        }
        let requester_did = request
            .did
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("discovery request is missing a requester DID"))?
            .to_string();
        let now = now_ts();
        home_people_discovery_send_acceptance(&data_dir, &context, &mut discovery, &request)
            .context("people discovery delivery failed")?;
        let _ = home_people_upsert_contact(
            &data_dir,
            &context,
            HomePeopleContactUpsert {
                peer_id: &request.peer_id,
                did: &requester_did,
                display_name: &request.display_name,
                handle: request.handle.as_deref(),
                source: "people_discovery",
                updated_at: now,
            },
        )?;
        if let Some(stored) = discovery.requests.get_mut(&request_id) {
            stored.status = "joined".to_string();
            stored.invite_id = None;
        }
        discovery.updated_at = now;
        home_save_people_discovery_state(&data_dir, &context, &discovery)?;
        home_people_discovery_summary(&data_dir, &context)
    })
    .await
    {
        Ok(Ok(discovery)) => Json(discovery).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn people_discovery_request_join(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    if let Err(err) = ensure_people_discovery_runtime_available(&data_dir).await {
        return home_error_response(anyhow::anyhow!(
            "managed local runtime could not start: {err}"
        ));
    }
    match tokio::task::spawn_blocking(move || {
        let request_id = request_id.trim().to_string();
        if request_id.is_empty() || request_id.len() > 256 {
            anyhow::bail!("discovery request id is invalid");
        }
        let mut discovery = home_people_discovery_state(&data_dir, &context)?;
        let request = discovery
            .requests
            .get(&request_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("discovery request not found"))?;
        if request.status != "accepted" && request.status != "joined" {
            anyhow::bail!("discovery request has no accepted invite to join");
        }
        let invite_id = request
            .invite_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("accepted discovery request is missing an invite"))?
            .to_string();
        let (_, local_did) = elastos_identity::load_or_create_did(&data_dir)?;
        let member = crate::room_service::accept_room_invite(
            &data_dir,
            crate::room_service::RoomInviteAcceptInput {
                actor_did: local_did.clone(),
                invite_id: invite_id.clone(),
            },
        )?;
        let member_profile = home_profile_card_summary_for_context(&data_dir, &context)
            .map(room_profile_card_from_home);
        let acceptance = crate::room_service::export_room_acceptance_envelope_with_profile(
            &data_dir,
            &invite_id,
            member_profile,
        )?;
        home_people_discovery_send_room_acceptance(
            &data_dir,
            &mut discovery,
            &request,
            &acceptance,
        )
        .context("people discovery delivery failed")?;
        if let Some(stored) = discovery.requests.get_mut(&request_id) {
            stored.status = "joined".to_string();
        }
        discovery.updated_at = now_ts();
        home_save_people_discovery_state(&data_dir, &context, &discovery)?;
        let summary = crate::room_service::load_summary(&data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(&data_dir, &summary);
        let discovery = home_people_discovery_summary(&data_dir, &context)?;
        Ok::<_, anyhow::Error>(PeopleDiscoveryRequestJoinResponse {
            schema: "elastos.people.discovery-join/v1".to_string(),
            status: "joined".to_string(),
            invite_id,
            member_did: member.member_did,
            discovery,
        })
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn services_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SERVICES_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        if let Err(err) = home_services_sync_access_decisions(&data_dir, &context) {
            tracing::warn!(
                error = %err,
                "could not sync Services access decisions"
            );
        }
        let mut home_state = home_state(&data_dir);
        apply_home_people_contacts_state(
            &data_dir,
            &context,
            &mut home_state.people,
            &mut home_state.services,
        )?;
        filter_removed_people_contacts(
            &data_dir,
            &context,
            &mut home_state.people,
            &mut home_state.services,
        )?;
        apply_home_services_selection(&data_dir, &context, &mut home_state.services)?;
        Ok::<_, anyhow::Error>(home_state.services)
    })
    .await
    {
        Ok(Ok(services)) => Json(services).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

pub(super) async fn services_offer_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<ServicesOfferUpdateRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SERVICES_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return home_error_response(err),
        };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        let mut home_state = home_state(&data_dir);
        apply_home_people_contacts_state(
            &data_dir,
            &context,
            &mut home_state.people,
            &mut home_state.services,
        )?;
        filter_removed_people_contacts(
            &data_dir,
            &context,
            &mut home_state.people,
            &mut home_state.services,
        )?;
        let offer_id = req.offer_id.trim();
        if offer_id.is_empty() {
            anyhow::bail!("service offer id is required");
        }
        let mut services_state = home_services_selection_state(&data_dir, &context)?;
        match req.section.trim() {
            "mine" => {
                if !home_state
                    .services
                    .local_offers
                    .iter()
                    .any(|offer| offer.offer_id == offer_id)
                {
                    anyhow::bail!("service offer is not available in Mine");
                }
                if req.selected {
                    services_state.local_offer_ids.insert(offer_id.to_string());
                } else {
                    services_state.local_offer_ids.remove(offer_id);
                }
            }
            "others" => {
                let Some(offer) = home_state
                    .services
                    .remote_offers
                    .iter()
                    .find(|offer| offer.offer_id == offer_id)
                else {
                    anyhow::bail!("service offer is not available in Others");
                };
                if offer.source == "configured_remote_exit" {
                    if req.selected {
                        services_state.remote_offer_ids.insert(offer_id.to_string());
                    } else {
                        anyhow::bail!(
                            "configured remote Exit grants are managed by Exit Provider config"
                        );
                    }
                } else if req.selected {
                    if offer.grant_required && !services_state.remote_offer_ids.contains(offer_id) {
                        let sent = home_services_send_access_request(&data_dir, &context, offer)
                            .map_err(|err| {
                                anyhow::anyhow!("service access request delivery failed: {err}")
                            })?;
                        services_state.remote_offer_requests.insert(
                            offer_id.to_string(),
                            HomeServicesRemoteOfferRequestRecord {
                                request_id: sent.request_id,
                                offer_id: offer_id.to_string(),
                                service_uri: offer.service_uri.clone(),
                                service_kind: offer.service_kind.clone(),
                                service_display_name: offer.display_name.clone(),
                                target_peer_id: sent.target_peer_id,
                                created_at: sent.created_at,
                                updated_at: sent.created_at,
                                status: "requested".to_string(),
                                installed_remote_exit_id: None,
                            },
                        );
                    }
                    services_state.remote_offer_ids.insert(offer_id.to_string());
                } else {
                    services_state.remote_offer_ids.remove(offer_id);
                    services_state.remote_offer_requests.remove(offer_id);
                }
            }
            _ => anyhow::bail!("service section must be mine or others"),
        }
        services_state.updated_at = now_ts();
        home_save_services_selection_state(&data_dir, &context, &services_state)?;
        apply_home_services_selection(&data_dir, &context, &mut home_state.services)?;
        Ok::<_, anyhow::Error>(home_state.services)
    })
    .await
    {
        Ok(Ok(services)) => Json(services).into_response(),
        Ok(Err(err)) => home_error_response(err),
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

fn home_services_selection_state_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/services-state.json",
        crate::auth::principal_localhost_root(&context.principal_id)
    )
}

fn home_services_selection_state_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<std::path::PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_services_selection_state_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid Services state root"))
}

fn home_services_default_selection_state(
    context: &HomeLaunchTokenContext,
) -> HomeServicesSelectionState {
    HomeServicesSelectionState {
        schema: HOME_SERVICES_STATE_SCHEMA.to_string(),
        principal_id: context.principal_id.clone(),
        localhost_root: crate::auth::principal_localhost_root(&context.principal_id),
        updated_at: 0,
        local_offer_ids: BTreeSet::new(),
        remote_offer_ids: BTreeSet::new(),
        remote_offer_requests: BTreeMap::new(),
    }
}

fn home_services_selection_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeServicesSelectionState> {
    let default_state = home_services_default_selection_state(context);
    let path = home_services_selection_state_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(default_state);
    };
    let raw = match crate::auth::read_principal_root_object(
        data_dir,
        &default_state.principal_id,
        &default_state.localhost_root,
        &home_services_selection_state_uri(context),
        &path,
    ) {
        Ok(raw) => raw,
        Err(err) if is_unencrypted_principal_root_state(&err) => return Ok(default_state),
        Err(err) if is_missing_principal_root_state_file(&err) => return Ok(default_state),
        Err(err) => return Err(err).with_context(|| format!("could not read {}", path.display())),
    };
    if raw.len() > HOME_SERVICES_STATE_MAX_BYTES {
        tracing::warn!(
            path = %path.display(),
            bytes = raw.len(),
            "ignored oversized Home services state"
        );
        return Ok(default_state);
    }
    let mut state: HomeServicesSelectionState = match serde_json::from_slice(&raw) {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "ignored invalid Home services state"
            );
            return Ok(default_state);
        }
    };
    if state.principal_id != default_state.principal_id
        || state.localhost_root != default_state.localhost_root
    {
        return Ok(default_state);
    }
    if state.schema.trim().is_empty() {
        state.schema = HOME_SERVICES_STATE_SCHEMA.to_string();
    } else if state.schema != HOME_SERVICES_STATE_SCHEMA {
        tracing::warn!(
            path = %path.display(),
            schema = %state.schema,
            "ignored unsupported Home services state schema"
        );
        return Ok(default_state);
    }
    Ok(state)
}

fn home_save_services_selection_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &HomeServicesSelectionState,
) -> anyhow::Result<()> {
    let path = home_services_selection_state_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(state)?;
    if raw.len() > HOME_SERVICES_STATE_MAX_BYTES {
        anyhow::bail!("Services state is too large");
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &state.principal_id,
        &state.localhost_root,
        &home_services_selection_state_uri(context),
        &path,
        &raw,
    )
    .with_context(|| format!("could not write {}", path.display()))
}

fn home_services_requests_state_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/services-requests.json",
        crate::auth::principal_localhost_root(&context.principal_id)
    )
}

fn home_services_requests_state_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<std::path::PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_services_requests_state_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid Services requests state root"))
}

fn home_services_default_requests_state(
    context: &HomeLaunchTokenContext,
) -> HomeServicesRequestsState {
    HomeServicesRequestsState {
        schema: HOME_SERVICES_REQUESTS_SCHEMA.to_string(),
        principal_id: context.principal_id.clone(),
        localhost_root: crate::auth::principal_localhost_root(&context.principal_id),
        updated_at: 0,
        requests: BTreeMap::new(),
    }
}

fn home_services_requests_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeServicesRequestsState> {
    let default_state = home_services_default_requests_state(context);
    let path = home_services_requests_state_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(default_state);
    }
    let raw = crate::auth::read_principal_root_object(
        data_dir,
        &default_state.principal_id,
        &default_state.localhost_root,
        &home_services_requests_state_uri(context),
        &path,
    )
    .with_context(|| format!("could not read {}", path.display()))?;
    if raw.len() > HOME_SERVICES_REQUESTS_MAX_BYTES {
        anyhow::bail!("Services requests state is too large");
    }
    let mut state: HomeServicesRequestsState = serde_json::from_slice(&raw)
        .with_context(|| format!("invalid Services requests state at {}", path.display()))?;
    if state.principal_id != default_state.principal_id
        || state.localhost_root != default_state.localhost_root
    {
        return Ok(default_state);
    }
    if state.schema.trim().is_empty() {
        state.schema = HOME_SERVICES_REQUESTS_SCHEMA.to_string();
    }
    Ok(state)
}

fn home_save_services_requests_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &HomeServicesRequestsState,
) -> anyhow::Result<()> {
    let path = home_services_requests_state_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(state)?;
    if raw.len() > HOME_SERVICES_REQUESTS_MAX_BYTES {
        anyhow::bail!("Services requests state is too large");
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &state.principal_id,
        &state.localhost_root,
        &home_services_requests_state_uri(context),
        &path,
        &raw,
    )
    .with_context(|| format!("could not write {}", path.display()))
}

fn home_services_contact_record_for_offer(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    offer: &HomeServiceOfferSummary,
) -> anyhow::Result<HomePeopleContactRecord> {
    let contact_id = offer
        .contact_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("service offer is not tied to a person"))?;
    let contacts = home_people_contacts_state(data_dir, context)?;
    contacts
        .contacts
        .get(contact_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("service offer person is not connected through People"))
}

fn home_services_new_access_request_id(target_peer_id: &str) -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|err| anyhow::anyhow!("service request id rng: {err}"))?;
    Ok(format!(
        "service-request:{}:{}",
        target_peer_id,
        hex::encode(bytes)
    ))
}

fn home_services_request_id_is_valid(request_id: &str) -> bool {
    let value = request_id.trim();
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == ':' || ch == '-' || ch == '_')
}

fn home_services_payload_text(
    payload: &serde_json::Value,
    field: &str,
    max_len: usize,
) -> Option<String> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= max_len)
        .map(ToOwned::to_owned)
}

fn home_services_local_exit_shared(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<bool> {
    Ok(home_services_selection_state(data_dir, context)?
        .local_offer_ids
        .contains(HOME_BROWSER_EXIT_LOCAL_OFFER_ID))
}

fn home_services_send_access_request(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    offer: &HomeServiceOfferSummary,
) -> anyhow::Result<HomeServiceAccessRequestSent> {
    if offer.service_kind != HOME_REMOTE_EXIT_SERVICE_KIND
        || offer.service_uri != HOME_BROWSER_EXIT_PEER_SERVICE_URI
    {
        anyhow::bail!("only Browser Exit service requests are supported");
    }
    let contact = home_services_contact_record_for_offer(data_dir, context, offer)?;
    let target_peer_id = contact.peer_id.trim();
    if target_peer_id.is_empty() {
        anyhow::bail!("service offer person has no Carrier peer route");
    }
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_SERVICES_REQUESTS_TOPIC, "dht")?;
    home_people_discovery_join_known_peers(
        &runtime,
        HOME_SERVICES_REQUESTS_TOPIC,
        &[target_peer_id.to_string()],
    )?;
    let requester_did = home_people_local_did(data_dir)?;
    let profile = home_profile_card_summary_for_context(data_dir, context);
    let requester_display_name = profile
        .as_ref()
        .map(|profile| profile.display_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ElastOS user".to_string());
    let requester_handle = profile.as_ref().and_then(|profile| profile.handle.clone());
    let created_at = now_ts();
    let request_id = home_services_new_access_request_id(target_peer_id)?;
    let payload = serde_json::json!({
        "schema": "elastos.service-access-request/v1",
        "kind": "service_access_request",
        "request_id": &request_id,
        "target_peer_id": target_peer_id,
        "requester_peer_id": &runtime.peer_id,
        "requester_did": requester_did,
        "requester_principal_id": &context.principal_id,
        "display_name": requester_display_name,
        "handle": requester_handle,
        "offer_id": &offer.offer_id,
        "service_uri": &offer.service_uri,
        "service_kind": &offer.service_kind,
        "service_display_name": &offer.display_name,
        "grant_scope": &offer.grant_scope,
        "created_at": created_at,
    });
    let delivery = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": HOME_SERVICES_REQUESTS_TOPIC,
            "sender_id": &runtime.peer_id,
            "sender": requester_display_name,
            "message": payload.to_string(),
            "ts": created_at,
        }),
    )?;
    home_services_require_remote_request_delivery(&delivery)?;
    Ok(HomeServiceAccessRequestSent {
        request_id,
        target_peer_id: target_peer_id.to_string(),
        created_at,
    })
}

fn home_services_require_remote_request_delivery(
    response: &serde_json::Value,
) -> anyhow::Result<()> {
    let remote_peer_count = response
        .get("data")
        .and_then(|data| data.get("remote_peer_count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let local_only = response
        .get("broadcast")
        .and_then(serde_json::Value::as_str)
        == Some("local_only");
    if local_only || remote_peer_count == 0 {
        anyhow::bail!(
            "Carrier service access request was not delivered to the other person's device"
        );
    }
    Ok(())
}

fn home_services_send_access_decision(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    request: &HomeServiceAccessRequestRecord,
    decision: &str,
) -> anyhow::Result<()> {
    if !matches!(decision, "approved" | "denied") {
        anyhow::bail!("service access request decision is invalid");
    }
    let target_peer_id = request.requester_peer_id.trim();
    if target_peer_id.is_empty() {
        anyhow::bail!("service access request has no requester peer route");
    }
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_SERVICES_REQUESTS_TOPIC, "dht")?;
    home_people_discovery_join_known_peers(
        &runtime,
        HOME_SERVICES_REQUESTS_TOPIC,
        &[target_peer_id.to_string()],
    )?;
    let profile = home_profile_card_summary_for_context(data_dir, context);
    let provider_display_name = profile
        .as_ref()
        .map(|profile| profile.display_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ElastOS user".to_string());
    let created_at = now_ts();
    let mut payload = serde_json::json!({
        "schema": "elastos.service-access-decision/v1",
        "kind": "service_access_decision",
        "request_id": &request.request_id,
        "target_requester_peer_id": target_peer_id,
        "provider_peer_id": &runtime.peer_id,
        "service_uri": &request.service_uri,
        "service_kind": &request.service_kind,
        "service_display_name": &request.service_display_name,
        "decision": decision,
        "created_at": created_at,
    });
    if decision == "approved"
        && request.service_uri == HOME_BROWSER_EXIT_PEER_SERVICE_URI
        && request.service_kind == HOME_REMOTE_EXIT_SERVICE_KIND
    {
        payload["remote_exit_grant"] = serde_json::json!({
            "schema": "elastos.service.remote-exit-grant/v1",
            "id": home_services_remote_exit_id(&request.service_display_name, &request.request_id),
            "grant_id": home_services_remote_exit_grant_id(&request.request_id),
            "peer_did": &runtime.peer_id,
            "carrier_service": "elastos://exit/open_stream",
            "connect_ticket": &runtime.connect_ticket,
            "allowed_schemes": ["tcp", "tls"],
            "allowed_ports": [80, 443],
            "max_active_streams": 4,
            "max_active_streams_per_principal": 2,
        });
    }
    let delivery = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": HOME_SERVICES_REQUESTS_TOPIC,
            "sender_id": &runtime.peer_id,
            "sender": provider_display_name,
            "message": payload.to_string(),
            "ts": created_at,
        }),
    )?;
    home_services_require_remote_request_delivery(&delivery)
}

fn home_services_sync_access_decisions(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<()> {
    let mut state = home_services_selection_state(data_dir, context)?;
    if state.remote_offer_requests.is_empty() {
        return Ok(());
    }
    let known_peers = state
        .remote_offer_requests
        .values()
        .filter_map(|request| {
            let peer_id = request.target_peer_id.trim();
            (!peer_id.is_empty()).then(|| peer_id.to_string())
        })
        .collect::<BTreeSet<_>>();
    if known_peers.is_empty() {
        return Ok(());
    }
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_SERVICES_REQUESTS_TOPIC, "dht")?;
    home_people_discovery_join_known_peers(
        &runtime,
        HOME_SERVICES_REQUESTS_TOPIC,
        &known_peers.iter().cloned().collect::<Vec<_>>(),
    )?;
    let recv = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": HOME_SERVICES_REQUESTS_TOPIC,
            "consumer_id": format!("home-services-decisions:{}", context.principal_id),
            "skip_sender_id": runtime.peer_id,
            "limit": 64,
        }),
    )?;
    let messages = recv
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if messages.is_empty() {
        return Ok(());
    }
    let mut changed = false;
    for message in messages {
        let Some(content) = message.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        match home_services_merge_access_decision(
            data_dir,
            context,
            &mut state,
            &payload,
            &runtime.peer_id,
        ) {
            Ok(merged) => changed |= merged,
            Err(err) => tracing::warn!("service access decision ignored: {err}"),
        }
    }
    if changed {
        state.updated_at = now_ts();
        home_save_services_selection_state(data_dir, context, &state)?;
    }
    Ok(())
}

fn home_services_merge_access_decision(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &mut HomeServicesSelectionState,
    payload: &serde_json::Value,
    local_peer_id: &str,
) -> anyhow::Result<bool> {
    if payload.get("schema").and_then(serde_json::Value::as_str)
        != Some("elastos.service-access-decision/v1")
        || payload.get("kind").and_then(serde_json::Value::as_str)
            != Some("service_access_decision")
        || payload
            .get("target_requester_peer_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            != Some(local_peer_id)
    {
        return Ok(false);
    }
    let Some(request_id) = home_services_payload_text(payload, "request_id", 256) else {
        return Ok(false);
    };
    if !home_services_request_id_is_valid(&request_id) {
        return Ok(false);
    }
    let Some(provider_peer_id) = home_services_payload_text(payload, "provider_peer_id", 256)
    else {
        return Ok(false);
    };
    let Some(decision) = home_services_payload_text(payload, "decision", 32) else {
        return Ok(false);
    };
    if !matches!(decision.as_str(), "approved" | "denied") {
        return Ok(false);
    }
    let Some(service_uri) = home_services_payload_text(payload, "service_uri", 256) else {
        return Ok(false);
    };
    let Some(service_kind) = home_services_payload_text(payload, "service_kind", 128) else {
        return Ok(false);
    };
    let updated_at = payload
        .get("created_at")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(now_ts);
    let Some(record) = state.remote_offer_requests.values_mut().find(|record| {
        record.request_id == request_id
            && record.target_peer_id == provider_peer_id
            && record.service_uri == service_uri
            && record.service_kind == service_kind
    }) else {
        return Ok(false);
    };
    let mut installed_remote_exit_id = record.installed_remote_exit_id.clone();
    if decision == "approved" {
        if service_uri == HOME_BROWSER_EXIT_PEER_SERVICE_URI
            && service_kind == HOME_REMOTE_EXIT_SERVICE_KIND
        {
            installed_remote_exit_id = Some(home_services_install_remote_exit_grant(
                data_dir, context, record, payload,
            )?);
        }
    } else if decision == "denied" {
        if let Some(installed_id) = record.installed_remote_exit_id.as_deref() {
            home_services_remove_remote_exit_grant(data_dir, installed_id)?;
        }
        installed_remote_exit_id = None;
    }
    if record.status == decision
        && record.updated_at == updated_at
        && record.installed_remote_exit_id == installed_remote_exit_id
    {
        return Ok(false);
    }
    record.status = decision;
    record.updated_at = updated_at;
    record.installed_remote_exit_id = installed_remote_exit_id;
    Ok(true)
}

fn home_services_remote_exit_id(display_name: &str, request_id: &str) -> String {
    let label = display_name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let digest = Sha256::digest(request_id.as_bytes());
    let suffix = hex::encode(&digest[..8]);
    if label.is_empty() {
        format!("services-remote-exit-{suffix}")
    } else {
        format!("services-{label}-{suffix}")
    }
}

fn home_services_remote_exit_grant_id(request_id: &str) -> String {
    let digest = Sha256::digest(request_id.as_bytes());
    format!("services-remote-exit-grant-{}", hex::encode(&digest[..8]))
}

fn home_services_exit_provider_config_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("config/exit-provider.json")
}

fn home_services_install_remote_exit_grant(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    record: &HomeServicesRemoteOfferRequestRecord,
    payload: &serde_json::Value,
) -> anyhow::Result<String> {
    let grant = payload
        .get("remote_exit_grant")
        .filter(|grant| {
            grant.get("schema").and_then(serde_json::Value::as_str)
                == Some("elastos.service.remote-exit-grant/v1")
        })
        .ok_or_else(|| {
            anyhow::anyhow!("approved Browser Exit decision did not include a remote Exit grant")
        })?;
    let connect_ticket = home_services_payload_text(
        grant,
        "connect_ticket",
        HOME_SERVICES_REMOTE_EXIT_TICKET_MAX_BYTES,
    )
    .ok_or_else(|| anyhow::anyhow!("approved Browser Exit grant is missing a Carrier ticket"))?;
    let provider_peer_id = home_services_payload_text(grant, "peer_did", 256)
        .or_else(|| home_services_payload_text(payload, "provider_peer_id", 256))
        .ok_or_else(|| anyhow::anyhow!("approved Browser Exit grant is missing a provider peer"))?;
    let id = home_services_remote_exit_id(&record.service_display_name, &record.request_id);
    let grant_id = home_services_remote_exit_grant_id(&record.request_id);
    let path = home_services_exit_provider_config_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut config = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if config.get("schema").is_none() {
        config["schema"] = serde_json::json!("elastos.browser.local-exit.config/v1");
    }
    let mut exits = config
        .get("remote_carrier_exits")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    exits.retain(|exit| {
        exit.get("id").and_then(serde_json::Value::as_str) != Some(id.as_str())
            && exit.get("grant_id").and_then(serde_json::Value::as_str) != Some(grant_id.as_str())
    });
    exits.push(serde_json::json!({
        "id": &id,
        "grant_id": &grant_id,
        "peer_did": provider_peer_id,
        "carrier_service": "elastos://exit/open_stream",
        "connect_ticket": connect_ticket,
        "allowed_principals": [context.principal_id.as_str()],
        "allowed_hosts": ["*"],
        "allowed_schemes": ["tcp", "tls"],
        "allowed_ports": [80, 443],
        "max_active_streams": 4,
        "max_active_streams_per_principal": 2,
    }));
    config["remote_carrier_exits"] = serde_json::Value::Array(exits);
    std::fs::write(&path, serde_json::to_vec_pretty(&config)?)?;
    Ok(id)
}

fn home_services_remove_remote_exit_grant(
    data_dir: &std::path::Path,
    installed_id: &str,
) -> anyhow::Result<()> {
    let path = home_services_exit_provider_config_path(data_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut config: serde_json::Value = serde_json::from_str(&raw)?;
    let Some(exits) = config
        .get("remote_carrier_exits")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    let filtered = exits
        .iter()
        .filter(|exit| exit.get("id").and_then(serde_json::Value::as_str) != Some(installed_id))
        .cloned()
        .collect::<Vec<_>>();
    config["remote_carrier_exits"] = serde_json::Value::Array(filtered);
    std::fs::write(&path, serde_json::to_vec_pretty(&config)?)?;
    Ok(())
}

pub(super) fn home_services_sync_access_requests(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<()> {
    if !home_services_local_exit_shared(data_dir, context)? {
        return Ok(());
    }
    let contacts = home_people_contacts_state(data_dir, context)?;
    let known_peers = contacts
        .contacts
        .values()
        .filter_map(|contact| {
            let peer_id = contact.peer_id.trim();
            (!peer_id.is_empty()).then(|| peer_id.to_string())
        })
        .collect::<BTreeSet<_>>();
    if known_peers.is_empty() {
        return Ok(());
    }
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_SERVICES_REQUESTS_TOPIC, "dht")?;
    home_people_discovery_join_known_peers(
        &runtime,
        HOME_SERVICES_REQUESTS_TOPIC,
        &known_peers.iter().cloned().collect::<Vec<_>>(),
    )?;
    let recv = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": HOME_SERVICES_REQUESTS_TOPIC,
            "consumer_id": format!("home-services-requests:{}", context.principal_id),
            "skip_sender_id": runtime.peer_id,
            "limit": 64,
        }),
    )?;
    let messages = recv
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if messages.is_empty() {
        return Ok(());
    }
    let mut state = home_services_requests_state(data_dir, context)?;
    let mut changed = false;
    for message in messages {
        let Some(content) = message.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        changed |= home_services_merge_access_request(
            &mut state,
            &known_peers,
            &payload,
            &runtime.peer_id,
        );
    }
    while state.requests.len() > 64 {
        let Some(oldest) = state
            .requests
            .iter()
            .min_by_key(|(_, request)| request.created_at)
            .map(|(request_id, _)| request_id.clone())
        else {
            break;
        };
        state.requests.remove(&oldest);
        changed = true;
    }
    if changed {
        state.updated_at = now_ts();
        home_save_services_requests_state(data_dir, context, &state)?;
    }
    Ok(())
}

fn home_services_merge_access_request(
    state: &mut HomeServicesRequestsState,
    known_peers: &BTreeSet<String>,
    payload: &serde_json::Value,
    local_peer_id: &str,
) -> bool {
    if payload.get("schema").and_then(serde_json::Value::as_str)
        != Some("elastos.service-access-request/v1")
        || payload.get("kind").and_then(serde_json::Value::as_str) != Some("service_access_request")
        || payload
            .get("target_peer_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            != Some(local_peer_id)
    {
        return false;
    }
    let Some(request_id) = home_services_payload_text(payload, "request_id", 256) else {
        return false;
    };
    if !home_services_request_id_is_valid(&request_id) {
        return false;
    }
    let Some(requester_peer_id) = home_services_payload_text(payload, "requester_peer_id", 256)
    else {
        return false;
    };
    if !known_peers.contains(&requester_peer_id) {
        return false;
    }
    let Some(service_uri) = home_services_payload_text(payload, "service_uri", 256) else {
        return false;
    };
    let Some(service_kind) = home_services_payload_text(payload, "service_kind", 128) else {
        return false;
    };
    if service_uri != HOME_BROWSER_EXIT_PEER_SERVICE_URI
        || service_kind != HOME_REMOTE_EXIT_SERVICE_KIND
    {
        return false;
    }
    let handle = clean_people_payload_handle(payload);
    let requester_display_name =
        clean_people_payload_display_name(payload, handle.as_deref(), None);
    let now = now_ts();
    let created_at = payload
        .get("created_at")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(now);
    let existing_status = state
        .requests
        .get(&request_id)
        .map(|request| request.status.clone());
    let status = match existing_status.as_deref() {
        Some("approved") | Some("denied") => existing_status.unwrap_or_default(),
        _ => "pending".to_string(),
    };
    let record = HomeServiceAccessRequestRecord {
        request_id: request_id.clone(),
        offer_id: home_services_payload_text(payload, "offer_id", 256).unwrap_or_default(),
        service_uri,
        service_kind,
        service_display_name: home_services_payload_text(payload, "service_display_name", 256)
            .unwrap_or_else(|| "Browser Exit Node".to_string()),
        requester_peer_id,
        requester_did: home_services_payload_text(payload, "requester_did", 256),
        requester_principal_id: home_services_payload_text(payload, "requester_principal_id", 256),
        requester_display_name,
        requester_handle: handle,
        created_at,
        updated_at: now,
        status,
    };
    let changed = state
        .requests
        .get(&request_id)
        .map(|existing| {
            existing.offer_id != record.offer_id
                || existing.service_uri != record.service_uri
                || existing.service_kind != record.service_kind
                || existing.service_display_name != record.service_display_name
                || existing.requester_peer_id != record.requester_peer_id
                || existing.requester_did != record.requester_did
                || existing.requester_principal_id != record.requester_principal_id
                || existing.requester_display_name != record.requester_display_name
                || existing.requester_handle != record.requester_handle
                || existing.status != record.status
        })
        .unwrap_or(true);
    if changed {
        state.requests.insert(request_id, record);
    }
    changed
}

pub(super) fn append_home_service_access_notifications(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    notifications: &mut HomeNotificationsSummary,
) {
    let Ok(state) = home_services_requests_state(data_dir, context) else {
        return;
    };
    let mut requests = state
        .requests
        .values()
        .filter(|request| request.status == "pending")
        .cloned()
        .collect::<Vec<_>>();
    requests.sort_by_key(|request| request.created_at);
    let existing_ids = notifications
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<BTreeSet<_>>();
    for request in requests {
        let id = format!("service-access-request:{}", request.request_id);
        if existing_ids.contains(&id) {
            continue;
        }
        notifications.unread_count += 1;
        notifications.attention_count += 1;
        notifications.entries.push(HomeNotificationEntrySummary {
            id,
            source_app: SERVICES_CAPSULE_ID.to_string(),
            kind: "service_access_request".to_string(),
            title: format!(
                "{} requests your Browser Exit Node",
                request.requester_display_name
            ),
            body: format!(
                "{} wants to use {}. Approval records your intent; Browser access still requires an installed remote Exit grant.",
                request.requester_display_name, request.service_display_name
            ),
            action_ref: Some(HomeNotificationActionSummary {
                app: SERVICES_CAPSULE_ID.to_string(),
                action_id: format!("service-approve-request:{}", request.request_id),
            }),
            severity: "attention".to_string(),
            read: false,
            created_at: request.created_at,
        });
    }
}

pub(super) fn approve_home_service_access_request(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> anyhow::Result<String> {
    home_services_mark_access_request(data_dir, context, request_id, "approved")
}

pub(super) fn deny_home_service_access_request(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    request_id: &str,
) -> anyhow::Result<String> {
    home_services_mark_access_request(data_dir, context, request_id, "denied")
}

fn home_services_mark_access_request(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    request_id: &str,
    status: &str,
) -> anyhow::Result<String> {
    let request_id = request_id.trim();
    if !home_services_request_id_is_valid(request_id) {
        anyhow::bail!("service request id is invalid");
    }
    let mut state = home_services_requests_state(data_dir, context)?;
    let Some(request) = state.requests.get(request_id).cloned() else {
        anyhow::bail!("service request not found");
    };
    home_services_send_access_decision(data_dir, context, &request, status)
        .with_context(|| "service access request delivery failed")?;
    let Some(request) = state.requests.get_mut(request_id) else {
        anyhow::bail!("service request not found");
    };
    request.status = status.to_string();
    request.updated_at = now_ts();
    let requester = request.requester_display_name.clone();
    state.updated_at = request.updated_at;
    home_save_services_requests_state(data_dir, context, &state)?;
    Ok(match status {
        "approved" => format!(
            "Approved Browser Exit request from {requester}. A private remote Exit grant was sent to the requester."
        ),
        "denied" => format!("Denied Browser Exit request from {requester}."),
        _ => "Updated service request.".to_string(),
    })
}

fn apply_home_services_selection(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    services: &mut HomeServicesSummary,
) -> anyhow::Result<()> {
    let state = home_services_selection_state(data_dir, context)?;
    let local_offers = std::mem::take(&mut services.local_offers);
    let remote_offers = std::mem::take(&mut services.remote_offers);

    let (local_offers, available_local_offers) =
        partition_service_offers(local_offers, &state.local_offer_ids);
    let (mut remote_offers, available_remote_offers) =
        partition_service_offers(remote_offers, &state.remote_offer_ids);
    for offer in &mut remote_offers {
        if let Some(request) = state.remote_offer_requests.get(&offer.offer_id) {
            let installed =
                request.status == "approved" && request.installed_remote_exit_id.is_some();
            offer.status = match request.status.as_str() {
                "approved" if installed => "active",
                "approved" => "approved",
                "denied" => "denied",
                _ => "requested",
            }
            .to_string();
            offer.enabled = installed;
            if installed {
                offer.grant_required = false;
                offer.grant_scope = "installed_remote_carrier_exit_grant".to_string();
                offer.route = Some("/apps/browser/".to_string());
            }
        }
    }

    services.local_offer_count = local_offers.len();
    services.remote_offer_count = remote_offers.len();
    services.available_local_offer_count = available_local_offers.len();
    services.available_remote_offer_count = available_remote_offers.len();
    services.local_offers = local_offers;
    services.remote_offers = remote_offers;
    services.available_local_offers = available_local_offers;
    services.available_remote_offers = available_remote_offers;
    Ok(())
}

fn partition_service_offers(
    offers: Vec<HomeServiceOfferSummary>,
    selected_ids: &BTreeSet<String>,
) -> (Vec<HomeServiceOfferSummary>, Vec<HomeServiceOfferSummary>) {
    let mut selected = Vec::new();
    let mut available = Vec::new();
    for offer in offers {
        if selected_ids.contains(&offer.offer_id) || offer.source == "configured_remote_exit" {
            selected.push(offer);
        } else {
            available.push(available_service_offer(offer));
        }
    }
    (selected, available)
}

fn available_service_offer(mut offer: HomeServiceOfferSummary) -> HomeServiceOfferSummary {
    offer.enabled = false;
    if matches!(offer.status.as_str(), "enabled" | "configured") {
        offer.status = "available".to_string();
    }
    offer
}

fn home_people_contact_exists(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    summary: &crate::room_service::RoomSummary,
    local_did: Option<&str>,
    contact_id: &str,
) -> anyhow::Result<bool> {
    if summary
        .room_control
        .members
        .iter()
        .filter(|member| local_did != Some(member.member_did.as_str()))
        .any(|member| home_people_contact_id(&member.member_did) == contact_id)
    {
        return Ok(true);
    }
    Ok(home_people_contacts_state(data_dir, context)?
        .contacts
        .contains_key(contact_id))
}

pub(super) async fn home_events(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(query): Query<HomeEventsQuery>,
) -> Response {
    let authority = match require_home_runtime_wallet_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return home_error_response(err),
    };
    let context = authority.home_launch_context();
    let previous_cursor = query.cursor.unwrap_or_default();
    let wait_ms = query
        .wait_ms
        .unwrap_or(HOME_EVENTS_DEFAULT_WAIT_MS)
        .min(HOME_EVENTS_MAX_WAIT_MS);
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let snapshot = home_realtime_snapshot(&state, &context, &authority).await;
        let cursor = home_realtime_cursor(&snapshot);
        if previous_cursor.trim().is_empty() || cursor != previous_cursor {
            let events = home_realtime_events(&previous_cursor, &snapshot);
            return Json(HomeEventsResponse {
                schema: HOME_EVENTS_SCHEMA.to_string(),
                cursor,
                keepalive: false,
                retry_after_ms: HOME_EVENTS_RETRY_MS,
                events,
            })
            .into_response();
        }
        if wait_ms == 0 || std::time::Instant::now() >= deadline {
            return Json(HomeEventsResponse {
                schema: HOME_EVENTS_SCHEMA.to_string(),
                cursor,
                keepalive: true,
                retry_after_ms: HOME_EVENTS_RETRY_MS,
                events: Vec::new(),
            })
            .into_response();
        }
        tokio::time::sleep(Duration::from_millis(HOME_EVENTS_POLL_MS)).await;
    }
}

pub(super) async fn home_events_stream(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let authority = match require_home_runtime_wallet_authority(&state.data_dir, &headers) {
        Ok(authority) => authority,
        Err(err) => return home_error_response(err),
    };
    let context = authority.home_launch_context();
    let stream_state = HomeEventsStreamState {
        state,
        context,
        authority,
        cursor: String::new(),
    };
    let stream = futures_lite::stream::unfold(stream_state, |mut stream_state| async move {
        loop {
            let snapshot = home_realtime_snapshot(
                &stream_state.state,
                &stream_state.context,
                &stream_state.authority,
            )
            .await;
            let cursor = home_realtime_cursor(&snapshot);
            if stream_state.cursor.is_empty() {
                stream_state.cursor = cursor;
            } else if cursor != stream_state.cursor {
                let events = home_realtime_events(&stream_state.cursor, &snapshot);
                stream_state.cursor = cursor.clone();
                let response = HomeEventsResponse {
                    schema: HOME_EVENTS_SCHEMA.to_string(),
                    cursor,
                    keepalive: false,
                    retry_after_ms: HOME_EVENTS_RETRY_MS,
                    events,
                };
                return Some((
                    Ok::<SseEvent, Infallible>(home_events_sse_event(response)),
                    stream_state,
                ));
            }
            tokio::time::sleep(Duration::from_millis(HOME_EVENTS_POLL_MS)).await;
        }
    });

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(HOME_EVENTS_STREAM_KEEPALIVE_SECS))
                .text("keepalive"),
        )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-accel-buffering"),
        axum::http::HeaderValue::from_static("no"),
    );
    response
}

struct HomeEventsStreamState {
    state: GatewayState,
    context: HomeLaunchTokenContext,
    authority: RuntimeWalletAuthority,
    cursor: String,
}

fn home_events_sse_event(response: HomeEventsResponse) -> SseEvent {
    let data = serde_json::to_string(&response).unwrap_or_else(|_| {
        format!(
            r#"{{"schema":"{}","cursor":"","keepalive":true,"retry_after_ms":{},"events":[]}}"#,
            HOME_EVENTS_SCHEMA, HOME_EVENTS_RETRY_MS
        )
    });
    SseEvent::default().event("runtime-events").data(data)
}

async fn home_realtime_snapshot(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
) -> HomeRealtimeSnapshot {
    let data_dir = state.data_dir.clone();
    let mut home_state = tokio::task::spawn_blocking(move || home_state(&data_dir))
        .await
        .unwrap_or_default();
    if apply_home_people_contacts_state(
        &state.data_dir,
        context,
        &mut home_state.people,
        &mut home_state.services,
    )
    .is_err()
    {
        home_state.people = HomePeopleSummary::default();
        home_state.services = HomeServicesSummary::default();
    }
    if filter_removed_people_contacts(
        &state.data_dir,
        context,
        &mut home_state.people,
        &mut home_state.services,
    )
    .is_err()
    {
        home_state.people = HomePeopleSummary::default();
        home_state.services = HomeServicesSummary::default();
    }
    if apply_home_services_selection(&state.data_dir, context, &mut home_state.services).is_err() {
        home_state.services = HomeServicesSummary::default();
    }
    let room_signature = home_room_realtime_signature(&home_state.room);
    let mut notifications = home_state.notifications;
    let wallet_approvals = system_wallet_approvals_summary(state, authority, false).await;
    let mut wallet_request_signature = wallet_approvals
        .approval_requests
        .iter()
        .map(|request| {
            format!(
                "{}:{}:{}:{}",
                request.request_id, request.status, request.intent, request.expires_at
            )
        })
        .collect::<Vec<_>>();
    wallet_request_signature.sort();
    append_wallet_approval_notifications(&mut notifications, wallet_approvals.approval_requests);
    let capability_requests = runtime_capability_pending_requests(&state.data_dir)
        .await
        .unwrap_or_default();
    let capability_request_count = capability_requests.len();
    append_runtime_capability_notifications(&mut notifications, capability_requests);
    append_home_service_access_notifications(&state.data_dir, context, &mut notifications);
    let mut notification_signature = notifications
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{}:{}:{}:{}",
                entry.id, entry.kind, entry.severity, entry.read
            )
        })
        .collect::<Vec<_>>();
    notification_signature.sort();
    let browser_sessions = super::gateway_browser::browser_gateway_session_status(
        &state.data_dir,
        &context.principal_id,
        None,
    )
    .await;
    let desktop_signature = home_desktop_events_signature(state, context).await;
    let people_signature = home_people_realtime_signature(&home_state.people);
    let services_signature = home_services_realtime_signature(&home_state.services);
    HomeRealtimeSnapshot {
        principal_id: context.principal_id.clone(),
        notification_signature,
        wallet_request_signature,
        capability_request_count,
        desktop_signature,
        room_signature,
        people_signature,
        services_signature,
        browser_sessions,
    }
}

fn home_realtime_cursor(snapshot: &HomeRealtimeSnapshot) -> String {
    let parts = home_realtime_cursor_parts(snapshot);
    format!(
        "v1:home={};inbox={};wallet={};browser={};desktop={};chat-room={};people={};services={}",
        parts.home,
        parts.inbox,
        parts.wallet,
        parts.browser,
        parts.desktop,
        parts.chat_room,
        parts.people,
        parts.services
    )
}

struct HomeRealtimeCursorParts {
    home: String,
    inbox: String,
    wallet: String,
    browser: String,
    desktop: String,
    chat_room: String,
    people: String,
    services: String,
}

fn home_realtime_cursor_parts(snapshot: &HomeRealtimeSnapshot) -> HomeRealtimeCursorParts {
    HomeRealtimeCursorParts {
        home: stable_cursor_hash(&snapshot.principal_id),
        inbox: stable_cursor_hash(&(
            &snapshot.notification_signature,
            &snapshot.wallet_request_signature,
            snapshot.capability_request_count,
        )),
        wallet: stable_cursor_hash(&snapshot.wallet_request_signature),
        browser: stable_cursor_hash(&snapshot.browser_sessions),
        desktop: stable_cursor_hash(&snapshot.desktop_signature),
        chat_room: stable_cursor_hash(&snapshot.room_signature),
        people: stable_cursor_hash(&snapshot.people_signature),
        services: stable_cursor_hash(&snapshot.services_signature),
    }
}

fn stable_cursor_hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    lowercase_hex(&Sha256::digest(bytes))
}

fn home_room_realtime_signature(room: &HomeRoomSummary) -> String {
    let mut pending = room
        .pending_requests
        .iter()
        .map(|request| format!("{}:{}", request.request_id, request.requested_at))
        .collect::<Vec<_>>();
    pending.sort();
    let mut sessions = room
        .active_sessions
        .iter()
        .map(|session| {
            // `last_seen_at` is heartbeat metadata. Including it here turns
            // routine refreshes into realtime events and can create a
            // poll/event feedback loop in Home-launched Chat windows.
            format!(
                "{}:{}:{}",
                session.display_name, session.device_label, session.approved_at
            )
        })
        .collect::<Vec<_>>();
    sessions.sort();
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        room.room_slug,
        room.pending_count,
        room.active_session_count,
        room.member_count,
        room.active_member_count,
        room.local_runtime_role.as_deref().unwrap_or_default(),
        pending.join(","),
        sessions.join(",")
    )
}

fn home_people_realtime_signature(people: &HomePeopleSummary) -> Vec<String> {
    let mut signature = people
        .contacts
        .iter()
        .map(|contact| {
            let profile = contact
                .profile_card
                .as_ref()
                .map(|card| {
                    format!(
                        "{}:{}:{}:{}:{}",
                        card.schema,
                        card.profile_id,
                        card.display_name,
                        card.handle.as_deref().unwrap_or_default(),
                        card.updated_at
                    )
                })
                .unwrap_or_default();
            format!(
                "{}:{}:{}:{}:{}:{}:{}",
                contact.contact_id,
                contact.display_name,
                contact.relationship,
                contact.route,
                contact.can_message,
                contact.device_label.as_deref().unwrap_or_default(),
                profile
            )
        })
        .collect::<Vec<_>>();
    signature.extend(people.service_offers.iter().map(|offer| {
        format!(
            "offer:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            offer.schema,
            offer.offer_id,
            offer.service_uri,
            offer.service_kind,
            offer.display_name,
            offer.provider_uri.as_deref().unwrap_or_default(),
            offer.provider_label,
            offer.policy_summary,
            offer.status,
            offer.enabled,
            offer.grant_required,
            offer.grant_scope.as_str(),
            offer.capsule_contract.as_str(),
            offer.contact_id.as_deref().unwrap_or_default(),
            offer.capsule_hint.as_deref().unwrap_or_default(),
            offer.route.as_deref().unwrap_or_default(),
        )
    }));
    signature.sort();
    signature
}

fn home_services_realtime_signature(services: &HomeServicesSummary) -> Vec<String> {
    let mut signature = services
        .local_offers
        .iter()
        .chain(services.remote_offers.iter())
        .chain(services.available_local_offers.iter())
        .chain(services.available_remote_offers.iter())
        .map(|offer| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
                offer.schema,
                offer.offer_id,
                offer.service_uri,
                offer.service_kind,
                offer.display_name,
                offer.provider_uri.as_deref().unwrap_or_default(),
                offer.provider_label,
                offer.status,
                offer.enabled,
                offer.grant_required,
                offer.grant_scope.as_str(),
                offer.capsule_contract.as_str(),
                offer.contact_id.as_deref().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

fn lowercase_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn home_realtime_events(
    previous_cursor: &str,
    snapshot: &HomeRealtimeSnapshot,
) -> Vec<HomeRealtimeEvent> {
    let previous = parse_home_realtime_cursor(previous_cursor);
    let current = home_realtime_cursor_parts(snapshot);
    let changed = [
        ("inbox", "inbox.changed", current.inbox),
        ("wallet", "wallet.requests.changed", current.wallet),
        ("browser", "browser.sessions.changed", current.browser),
        ("desktop", "home.desktop.changed", current.desktop),
        ("chat-room", "chat-room.changed", current.chat_room),
        ("people", "people.changed", current.people),
        ("services", "services.changed", current.services),
    ]
    .into_iter()
    .filter(|(scope, _kind, current_hash)| {
        previous
            .get(*scope)
            .map(|previous_hash| previous_hash != current_hash)
            .unwrap_or(true)
    })
    .collect::<Vec<_>>();
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let mut events = Vec::new();
    if previous.is_empty()
        || previous
            .get("home")
            .map(|previous_hash| previous_hash != &current.home)
            .unwrap_or(true)
    {
        events.push(HomeRealtimeEvent {
            kind: "home.summary.changed".to_string(),
            scope: "home".to_string(),
            at,
        });
    }
    events.extend(
        changed
            .into_iter()
            .map(|(scope, kind, _)| HomeRealtimeEvent {
                kind: kind.to_string(),
                scope: scope.to_string(),
                at,
            }),
    );
    events
}

fn parse_home_realtime_cursor(cursor: &str) -> BTreeMap<String, String> {
    let mut parsed = BTreeMap::new();
    let Some(rest) = cursor.strip_prefix("v1:") else {
        return parsed;
    };
    for part in rest.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.is_empty() || value.is_empty() {
            continue;
        }
        parsed.insert(key.to_string(), value.to_string());
    }
    parsed
}

pub(super) async fn home_browser_state_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_shell_state_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    match home_browser_state(&state.data_dir, &context) {
        Ok(state) => Json(state).into_response(),
        Err(err) => home_error_response(err),
    }
}

pub(super) async fn home_browser_state_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeBrowserStateUpdate>,
) -> Response {
    let context = match require_home_shell_state_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    match home_save_browser_state(&state.data_dir, &context, input) {
        Ok(state) => Json(state).into_response(),
        Err(err) => home_error_response(err),
    }
}

pub(super) async fn home_active_shell_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = require_home_active_shell_token_context(&state.data_dir, &headers).ok();
    match home_active_shell_summary(&state.data_dir, context.as_ref()) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => home_error_response(err),
    }
}

pub(super) async fn home_active_shell_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeActiveShellUpdate>,
) -> Response {
    let context = match require_home_active_shell_update_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    match home_save_active_shell(&state.data_dir, &context, input.active.trim()) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => {
            let text = err.to_string();
            let status = if text.contains("not a launchable shell") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, text).into_response()
        }
    }
}

fn require_home_active_shell_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    if let Ok(context) = require_home_token_context(data_dir, headers) {
        return Ok(context);
    }
    require_home_active_shell_update_token_context(data_dir, headers)
}

fn require_home_active_shell_wallet_authority(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<RuntimeWalletAuthority> {
    if let Ok(authority) = require_home_runtime_wallet_authority(data_dir, headers) {
        return Ok(authority);
    }
    let allowed = BTreeSet::from([
        HOME_CAPSULE_ID.to_string(),
        SYSTEM_CAPSULE_ID.to_string(),
        HOME_GUI_SHELL_ID.to_string(),
        HOME_CLI_SHELL_ID.to_string(),
    ]);
    let allowed_refs = allowed.iter().map(String::as_str).collect::<Vec<_>>();
    require_runtime_wallet_authority(data_dir, headers, &allowed_refs)
}

fn require_home_shell_state_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_context(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID, HOME_GUI_SHELL_ID, HOME_CLI_SHELL_ID],
    )
}

fn require_home_active_shell_update_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let allowed = BTreeSet::from([
        HOME_CAPSULE_ID.to_string(),
        SYSTEM_CAPSULE_ID.to_string(),
        HOME_GUI_SHELL_ID.to_string(),
        HOME_CLI_SHELL_ID.to_string(),
    ]);
    let allowed_refs = allowed.iter().map(String::as_str).collect::<Vec<_>>();
    require_home_launch_token_for_any_context(data_dir, headers, &allowed_refs)
}

fn home_authority_summary(context: &HomeLaunchTokenContext) -> HomeAuthoritySummary {
    HomeAuthoritySummary {
        signed_in: true,
        principal_id: context.principal_id.clone(),
        session_id: context.session_id.clone(),
        proof_binding_id: context.proof_binding_id.clone(),
        wallet_connected: context
            .proof_binding_id
            .as_deref()
            .is_some_and(|value| value.starts_with("proof:wallet:")),
    }
}

fn standard_home_identity_summary() -> HomeIdentitySummary {
    HomeIdentitySummary {
        device_did: None,
        handle: None,
        profile_card: None,
    }
}

pub(super) fn home_profile_card_summary_for_context(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> Option<HomeProfileCardSummary> {
    home_profile_card(data_dir, context).ok().flatten()
}

fn home_profile_card(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<Option<HomeProfileCardSummary>> {
    let path = home_profile_card_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_profile_card_uri(context),
        &path,
    )?;
    if bytes.len() > HOME_PROFILE_CARD_MAX_BYTES {
        anyhow::bail!("profile card object is too large");
    }
    let card: HomeProfileCardSummary = serde_json::from_slice(&bytes)?;
    validate_home_profile_card(context, card).map(Some)
}

fn home_save_profile_card(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    display_name: &str,
    updated_at: u64,
) -> anyhow::Result<HomeProfileCardSummary> {
    let display_name = crate::auth::clean_principal_display_name(Some(display_name))?
        .ok_or_else(|| anyhow::anyhow!("profile name must not be empty"))?;
    let card = HomeProfileCardSummary {
        schema: HOME_PROFILE_CARD_SCHEMA.to_string(),
        profile_id: home_profile_id(context),
        display_name,
        handle: None,
        updated_at,
    };
    let bytes = serde_json::to_vec_pretty(&card)?;
    if bytes.len() > HOME_PROFILE_CARD_MAX_BYTES {
        anyhow::bail!("profile card object is too large");
    }
    let path = home_profile_card_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_profile_card_uri(context),
        &path,
        &bytes,
    )?;
    Ok(card)
}

fn validate_home_profile_card(
    context: &HomeLaunchTokenContext,
    card: HomeProfileCardSummary,
) -> anyhow::Result<HomeProfileCardSummary> {
    if card.schema != HOME_PROFILE_CARD_SCHEMA {
        anyhow::bail!("unsupported profile card schema");
    }
    if card.profile_id != home_profile_id(context) {
        anyhow::bail!("profile card id mismatch");
    }
    let display_name = crate::auth::clean_principal_display_name(Some(&card.display_name))?
        .ok_or_else(|| anyhow::anyhow!("profile card display name must not be empty"))?;
    Ok(HomeProfileCardSummary {
        display_name,
        ..card
    })
}

fn home_profile_card_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Profile/profile-card.json",
        home_browser_localhost_root(context)
    )
}

fn home_profile_card_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_profile_card_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid profile card object path"))
}

fn home_profile_id(context: &HomeLaunchTokenContext) -> String {
    let digest = Sha256::digest(context.principal_id.as_bytes());
    format!("profile:local:{}", hex::encode(&digest[..12]))
}

fn standard_home_authority_summary() -> HomeAuthoritySummary {
    HomeAuthoritySummary {
        signed_in: false,
        ..HomeAuthoritySummary::default()
    }
}

fn standard_home_browser_state() -> HomeBrowserStateSummary {
    HomeBrowserStateSummary {
        schema: HOME_BROWSER_STATE_SCHEMA.to_string(),
        ..HomeBrowserStateSummary::default()
    }
}

fn home_active_shell_summary(
    data_dir: &std::path::Path,
    context: Option<&HomeLaunchTokenContext>,
) -> anyhow::Result<HomeActiveShellSummary> {
    let candidates = home_active_shell_candidates(data_dir);
    let saved = match context {
        Some(context) => home_active_shell_state(data_dir, context)?.map(|state| state.active),
        None => None,
    };
    let saved_canonical = saved
        .as_deref()
        .map(home_active_shell_saved_state_name)
        .filter(|candidate| !candidate.is_empty());
    let saved_is_valid = saved_canonical.as_ref().is_some_and(|candidate| {
        candidates
            .iter()
            .any(|shell| shell.name.as_str() == candidate.as_str())
    });
    let active = saved_canonical
        .clone()
        .filter(|_| saved_is_valid)
        .or_else(|| {
            candidates
                .iter()
                .find(|shell| shell.name == HOME_GUI_SHELL_ID)
                .map(|shell| shell.name.clone())
        })
        .or_else(|| candidates.first().map(|shell| shell.name.clone()))
        .unwrap_or_default();
    let needs_repair = saved.as_deref().is_some_and(|saved| saved.trim() != active);
    if needs_repair && !active.is_empty() {
        if let Some(context) = context {
            if let Err(err) = home_save_active_shell(data_dir, context, &active) {
                tracing::warn!(
                    active_shell = %active,
                    error = %err,
                    "failed to repair obsolete Home active shell state"
                );
            }
        }
    }
    Ok(HomeActiveShellSummary {
        schema: HOME_ACTIVE_SHELL_SCHEMA.to_string(),
        active,
        candidates,
    })
}

pub(super) fn home_active_shell_snapshot_value(
    data_dir: &std::path::Path,
    context: Option<&HomeLaunchTokenContext>,
) -> anyhow::Result<serde_json::Value> {
    serde_json::to_value(home_active_shell_summary(data_dir, context)?)
        .context("failed to serialize Home active shell snapshot")
}

fn home_active_shell_candidates(data_dir: &std::path::Path) -> Vec<HomeActiveShellCandidate> {
    let mut candidates = BTreeMap::<String, HomeActiveShellCandidate>::new();
    for capsule in capsule_catalog_summary(data_dir)
        .capsules
        .into_iter()
        .filter(|capsule| capsule.role == CapsuleRole::Shell && capsule.launchable)
        .filter(|capsule| capsule.name != HOME_CAPSULE_ID)
        .filter(|capsule| is_trusted_home_shell_id(&capsule.name))
    {
        let Some(catalog_route) = capsule.route else {
            continue;
        };
        let capsule_name = capsule.name;
        let is_home_gui = capsule_name == HOME_GUI_SHELL_ID;
        let name = capsule_name.clone();
        let candidate = HomeActiveShellCandidate {
            name: name.clone(),
            title: if is_home_gui {
                "Home GUI".to_string()
            } else {
                capsule.title
            },
            description: capsule.description,
            route: if is_home_gui {
                HOME_ROUTE.to_string()
            } else {
                catalog_route
            },
            role: capsule.role,
            launchable: capsule.launchable,
            trust_state: capsule.trust_state,
        };
        candidates.insert(name, candidate);
    }
    candidates.into_values().collect()
}

fn home_active_shell_saved_state_name(active: &str) -> String {
    let trimmed = active.trim();
    if trimmed == HOME_CAPSULE_ID {
        HOME_GUI_SHELL_ID.to_string()
    } else {
        trimmed.to_string()
    }
}

fn home_active_shell_state_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/active-shell.json",
        home_browser_localhost_root(context)
    )
}

fn home_active_shell_state_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_active_shell_state_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid Home active shell state root"))
}

fn home_active_shell_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<Option<HomeActiveShellState>> {
    let path = home_active_shell_state_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(None);
    }
    let principal_id = home_browser_principal_id(context);
    let localhost_root = home_browser_localhost_root(context);
    let bytes = match crate::auth::read_principal_root_object(
        data_dir,
        &principal_id,
        &localhost_root,
        &home_active_shell_state_uri(context),
        &path,
    ) {
        Ok(bytes) => bytes,
        Err(err) if is_unencrypted_principal_root_state(&err) => return Ok(None),
        Err(err) if is_missing_principal_root_state_file(&err) => return Ok(None),
        Err(err) => return Err(err),
    };
    if bytes.len() > HOME_ACTIVE_SHELL_MAX_BYTES {
        anyhow::bail!("Home active shell state is too large");
    }
    let state: HomeActiveShellState = match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "ignored invalid Home active shell state"
            );
            return Ok(None);
        }
    };
    if state.schema != HOME_ACTIVE_SHELL_SCHEMA
        || state.principal_id != principal_id
        || state.localhost_root != localhost_root
    {
        tracing::warn!(
            path = %path.display(),
            "ignored mismatched Home active shell state"
        );
        return Ok(None);
    }
    Ok(Some(state))
}

fn home_save_active_shell(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    active: &str,
) -> anyhow::Result<HomeActiveShellSummary> {
    let candidates = home_active_shell_candidates(data_dir);
    let active = active.trim();
    let Some(candidate) = candidates.iter().find(|candidate| candidate.name == active) else {
        anyhow::bail!("active shell is not a launchable shell");
    };
    let state = HomeActiveShellState {
        schema: HOME_ACTIVE_SHELL_SCHEMA.to_string(),
        principal_id: home_browser_principal_id(context),
        localhost_root: home_browser_localhost_root(context),
        active: candidate.name.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&state)?;
    if bytes.len() > HOME_ACTIVE_SHELL_MAX_BYTES {
        anyhow::bail!("Home active shell state is too large");
    }
    let path = home_active_shell_state_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_active_shell_state_uri(context),
        &path,
        &bytes,
    )?;
    Ok(HomeActiveShellSummary {
        schema: HOME_ACTIVE_SHELL_SCHEMA.to_string(),
        active: candidate.name.clone(),
        candidates,
    })
}

fn home_browser_principal_id(context: &HomeLaunchTokenContext) -> String {
    context.principal_id.clone()
}

fn home_browser_localhost_root(context: &HomeLaunchTokenContext) -> String {
    crate::auth::principal_localhost_root(&context.principal_id)
}

fn home_desktop_uri(context: &HomeLaunchTokenContext) -> String {
    format!("{}/Desktop", home_browser_localhost_root(context))
}

fn standard_home_desktop_objects_summary() -> HomeDesktopObjectsSummary {
    HomeDesktopObjectsSummary {
        schema: HOME_DESKTOP_OBJECTS_SCHEMA.to_string(),
        uri: String::new(),
        objects: Vec::new(),
        stale: false,
        error: None,
    }
}

async fn home_desktop_objects_summary(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> HomeDesktopObjectsSummary {
    let uri = home_desktop_uri(context);
    let Some(registry) = state.provider_registry.as_ref() else {
        return HomeDesktopObjectsSummary {
            schema: HOME_DESKTOP_OBJECTS_SCHEMA.to_string(),
            uri,
            objects: Vec::new(),
            stale: true,
            error: Some("object provider registry unavailable".to_string()),
        };
    };
    let request = serde_json::json!({
        "op": "list",
        "principal_id": &context.principal_id,
        "uri": uri,
    });
    let response = match registry.send_raw("object", &request).await {
        Ok(response) => response,
        Err(err) => {
            return HomeDesktopObjectsSummary {
                schema: HOME_DESKTOP_OBJECTS_SCHEMA.to_string(),
                uri,
                objects: Vec::new(),
                stale: true,
                error: Some(format!("object provider failed to list Desktop: {err}")),
            }
        }
    };
    if response.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return HomeDesktopObjectsSummary {
            schema: HOME_DESKTOP_OBJECTS_SCHEMA.to_string(),
            uri: request
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            objects: Vec::new(),
            stale: true,
            error: Some(
                response
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("object provider failed to list Desktop")
                    .to_string(),
            ),
        };
    }
    let data = response.get("data").and_then(serde_json::Value::as_object);
    let mut objects = data
        .and_then(|data| data.get("objects"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(trash) = home_trash_desktop_object(state, context).await {
        objects.push(trash);
    }
    let uri = data
        .and_then(|data| data.get("uri"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            request
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        })
        .to_string();
    HomeDesktopObjectsSummary {
        schema: HOME_DESKTOP_OBJECTS_SCHEMA.to_string(),
        uri,
        objects,
        stale: false,
        error: None,
    }
}

async fn home_trash_desktop_object(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Option<serde_json::Value> {
    let registry = state.provider_registry.as_ref()?;
    let request = serde_json::json!({
        "op": "roots",
        "principal_id": &context.principal_id,
    });
    let response = registry.send_raw("object", &request).await.ok()?;
    if response.get("status").and_then(serde_json::Value::as_str) != Some("ok") {
        return None;
    }
    let roots = response
        .get("data")
        .and_then(|data| data.get("roots"))
        .and_then(serde_json::Value::as_array)?;
    let trash = roots
        .iter()
        .find(|root| root.get("id").and_then(serde_json::Value::as_str) == Some("trash"))?;
    let uri = trash
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .filter(|uri| !uri.trim().is_empty())?;
    let metadata = trash.get("metadata").and_then(serde_json::Value::as_object);
    let empty = metadata
        .and_then(|metadata| metadata.get("empty"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let item_count = metadata
        .and_then(|metadata| metadata.get("item_count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Some(serde_json::json!({
        "uri": uri,
        "name": "Trash",
        "kind": "directory",
        "mime": "inode/directory",
        "capabilities": trash
            .get("capabilities")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(["open", "list", "properties"])),
        "metadata": {
            "schema": HOME_SYSTEM_DESKTOP_OBJECT_SCHEMA,
            "system_kind": "trash",
            "empty": empty,
            "item_count": item_count,
            "provider_root": trash,
        },
    }))
}

async fn home_desktop_events_signature(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Vec<String> {
    let Some(registry) = state.provider_registry.as_ref() else {
        return Vec::new();
    };
    let mut signature = Vec::new();
    for uri in [
        home_desktop_uri(context),
        format!("{}/.Trash", home_browser_localhost_root(context)),
    ] {
        let request = serde_json::json!({
            "op": "events",
            "principal_id": &context.principal_id,
            "uri": uri,
            "limit": 32,
        });
        let Ok(response) = registry.send_raw("object", &request).await else {
            continue;
        };
        let Some(events) = response
            .get("data")
            .and_then(|data| data.get("events"))
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        signature.extend(events.iter().map(|event| {
            format!(
                "{}:{}:{}",
                event
                    .get("event_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                event
                    .get("op")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                event
                    .get("at")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default()
            )
        }));
    }
    signature.sort();
    signature
}

fn default_home_browser_state(context: &HomeLaunchTokenContext) -> HomeBrowserStateSummary {
    HomeBrowserStateSummary {
        schema: HOME_BROWSER_STATE_SCHEMA.to_string(),
        principal_id: home_browser_principal_id(context),
        localhost_root: home_browser_localhost_root(context),
        ..HomeBrowserStateSummary::default()
    }
}

fn home_browser_state_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_browser_state_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid Home state root"))
}

fn home_browser_state_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/browser-state.json",
        home_browser_localhost_root(context)
    )
}

fn home_browser_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeBrowserStateSummary> {
    let path = home_browser_state_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(default_home_browser_state(context));
    }
    let principal_id = home_browser_principal_id(context);
    let localhost_root = home_browser_localhost_root(context);
    let bytes = match crate::auth::read_principal_root_object(
        data_dir,
        &principal_id,
        &localhost_root,
        &home_browser_state_uri(context),
        &path,
    ) {
        Ok(bytes) => bytes,
        Err(err) if is_unencrypted_principal_root_state(&err) => {
            return Ok(default_home_browser_state(context));
        }
        Err(err) if is_missing_principal_root_state_file(&err) => {
            return Ok(default_home_browser_state(context));
        }
        Err(err) => return Err(err),
    };
    let mut state: HomeBrowserStateSummary = match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "ignored invalid Home browser state"
            );
            return Ok(default_home_browser_state(context));
        }
    };
    if state.schema != HOME_BROWSER_STATE_SCHEMA {
        tracing::warn!(
            path = %path.display(),
            schema = %state.schema,
            "ignored unsupported Home browser state schema"
        );
        return Ok(default_home_browser_state(context));
    }
    if state.principal_id != principal_id {
        tracing::warn!(
            path = %path.display(),
            principal_id = %state.principal_id,
            "ignored Home browser state principal mismatch"
        );
        return Ok(default_home_browser_state(context));
    }
    if state.localhost_root != localhost_root {
        tracing::warn!(
            path = %path.display(),
            localhost_root = %state.localhost_root,
            "ignored Home browser state root mismatch"
        );
        return Ok(default_home_browser_state(context));
    }
    state.recent_targets = sanitize_recent_targets(state.recent_targets);
    sanitize_home_browser_state_targets(data_dir, &mut state);
    Ok(state)
}

fn is_unencrypted_principal_root_state(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .to_string()
            .contains(crate::auth::PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED)
    })
}

fn is_missing_principal_root_state_file(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

fn home_save_browser_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    input: HomeBrowserStateUpdate,
) -> anyhow::Result<HomeBrowserStateSummary> {
    let mut state = home_browser_state(data_dir, context)?;
    if let Some(layout) = input.layout {
        state.layout = layout;
    }
    if let Some(session) = input.session {
        state.session = session;
    }
    if let Some(recent_targets) = input.recent_targets {
        state.recent_targets = sanitize_recent_targets(recent_targets);
    }
    sanitize_home_browser_state_targets(data_dir, &mut state);
    let bytes = serde_json::to_vec_pretty(&state)?;
    if bytes.len() > HOME_BROWSER_STATE_MAX_BYTES {
        anyhow::bail!("Home browser state is too large");
    }
    let path = home_browser_state_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_browser_state_uri(context),
        &path,
        &bytes,
    )?;
    Ok(state)
}

fn default_home_people_contacts_state(context: &HomeLaunchTokenContext) -> HomePeopleContactsState {
    HomePeopleContactsState {
        schema: HOME_PEOPLE_CONTACTS_SCHEMA.to_string(),
        principal_id: home_browser_principal_id(context),
        localhost_root: home_browser_localhost_root(context),
        updated_at: 0,
        contacts: BTreeMap::new(),
    }
}

fn home_people_contacts_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/people-contacts.json",
        home_browser_localhost_root(context)
    )
}

fn home_people_contacts_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_people_contacts_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid People contacts state root"))
}

fn home_people_contacts_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomePeopleContactsState> {
    let path = home_people_contacts_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(default_home_people_contacts_state(context));
    }
    let principal_id = home_browser_principal_id(context);
    let localhost_root = home_browser_localhost_root(context);
    let bytes = match crate::auth::read_principal_root_object(
        data_dir,
        &principal_id,
        &localhost_root,
        &home_people_contacts_uri(context),
        &path,
    ) {
        Ok(bytes) => bytes,
        Err(err) if is_unencrypted_principal_root_state(&err) => {
            return Ok(default_home_people_contacts_state(context));
        }
        Err(err) if is_missing_principal_root_state_file(&err) => {
            return Ok(default_home_people_contacts_state(context));
        }
        Err(err) => return Err(err),
    };
    let state: HomePeopleContactsState = serde_json::from_slice(&bytes)?;
    if state.schema != HOME_PEOPLE_CONTACTS_SCHEMA {
        anyhow::bail!("unsupported People contacts schema");
    }
    if state.principal_id != principal_id {
        anyhow::bail!("People contacts principal mismatch");
    }
    if state.localhost_root != localhost_root {
        anyhow::bail!("People contacts root mismatch");
    }
    Ok(state)
}

fn home_save_people_contacts_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &HomePeopleContactsState,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if bytes.len() > HOME_PEOPLE_CONTACTS_MAX_BYTES {
        anyhow::bail!("People contacts state is too large");
    }
    let path = home_people_contacts_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_people_contacts_uri(context),
        &path,
        &bytes,
    )?;
    Ok(())
}

fn clean_people_person_display_name(display_name: Option<&str>, handle: Option<&str>) -> String {
    let cleaned_display = display_name.and_then(|value| {
        crate::auth::clean_principal_display_name(Some(value))
            .ok()
            .flatten()
    });
    if cleaned_display
        .as_deref()
        .is_some_and(|value| value != "ElastOS user")
    {
        return cleaned_display.unwrap_or_default();
    }
    clean_people_contact_handle(handle)
        .or(cleaned_display)
        .unwrap_or_else(|| "ElastOS user".to_string())
}

fn clean_people_contact_handle(handle: Option<&str>) -> Option<String> {
    handle
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(ToOwned::to_owned)
}

fn clean_people_payload_handle(payload: &serde_json::Value) -> Option<String> {
    clean_people_contact_handle(payload.get("handle").and_then(serde_json::Value::as_str))
}

fn clean_people_payload_display_name(
    payload: &serde_json::Value,
    handle: Option<&str>,
    fallback: Option<&str>,
) -> String {
    let display_name = payload
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .or(fallback);
    clean_people_person_display_name(display_name, handle)
}

fn home_people_discovery_request_visible(request: &HomePeopleDiscoveryRequestSummary) -> bool {
    matches!(request.status.as_str(), "incoming" | "requested")
}

struct HomePeopleContactUpsert<'a> {
    peer_id: &'a str,
    did: &'a str,
    display_name: &'a str,
    handle: Option<&'a str>,
    source: &'a str,
    updated_at: u64,
}

fn home_people_upsert_contact(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    upsert: HomePeopleContactUpsert<'_>,
) -> anyhow::Result<(String, bool)> {
    let peer_id = upsert.peer_id.trim();
    if peer_id.is_empty() || peer_id.len() > 256 {
        anyhow::bail!("People contact peer id is invalid");
    }
    let did = upsert.did.trim();
    if did.is_empty() || did.len() > 512 {
        anyhow::bail!("People contact DID is invalid");
    }
    let contact_id = home_people_contact_id(did);
    let mut state = home_people_contacts_state(data_dir, context)?;
    let removed_at = home_people_removed_contacts(data_dir, context)
        .ok()
        .and_then(|removed| removed.contacts.get(&contact_id).copied());
    let existing = state.contacts.get(&contact_id).cloned();
    let was_visible = existing.as_ref().is_some_and(|record| {
        removed_at
            .map(|removed_at| record.added_at > removed_at)
            .unwrap_or(true)
    });
    let added_at = existing
        .as_ref()
        .map(|record| record.added_at)
        .filter(|value| *value > 0)
        .filter(|value| {
            removed_at
                .map(|removed_at| *value > removed_at)
                .unwrap_or(true)
        })
        .unwrap_or(upsert.updated_at);
    let handle = clean_people_contact_handle(upsert.handle);
    let display_name =
        clean_people_person_display_name(Some(upsert.display_name), handle.as_deref());
    let record = HomePeopleContactRecord {
        contact_id: contact_id.clone(),
        peer_id: peer_id.to_string(),
        did: Some(did.to_string()),
        display_name,
        handle,
        added_at,
        updated_at: upsert.updated_at,
        source: upsert.source.to_string(),
    };
    let changed = existing
        .as_ref()
        .map(|existing| {
            existing.peer_id != record.peer_id
                || existing.did != record.did
                || existing.display_name != record.display_name
                || existing.handle != record.handle
                || existing.added_at != record.added_at
                || existing.updated_at != record.updated_at
                || existing.source != record.source
        })
        .unwrap_or(true);
    if changed {
        state.contacts.insert(contact_id.clone(), record);
        state.updated_at = upsert.updated_at;
        home_save_people_contacts_state(data_dir, context, &state)?;
    }
    Ok((contact_id, !was_visible))
}

fn home_people_contact_summary_from_record(
    record: &HomePeopleContactRecord,
) -> Option<HomePeopleContactSummary> {
    if record.contact_id.trim().is_empty() || record.peer_id.trim().is_empty() {
        return None;
    }
    Some(HomePeopleContactSummary {
        contact_id: record.contact_id.clone(),
        added_at: record.added_at,
        display_name: clean_people_person_display_name(
            Some(&record.display_name),
            record.handle.as_deref(),
        ),
        handle: record.handle.clone(),
        relationship: "connected".to_string(),
        route: format!("elastos://peer/{}", record.peer_id),
        can_message: false,
        device_label: Some(record.peer_id.clone()),
        profile_card: None,
        last_seen_at: Some(record.updated_at),
    })
}

pub(super) fn apply_home_people_contacts_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    people: &mut HomePeopleSummary,
    services: &mut HomeServicesSummary,
) -> anyhow::Result<()> {
    let state = home_people_contacts_state(data_dir, context)?;
    if state.contacts.is_empty() {
        return Ok(());
    }
    let mut contacts = people
        .contacts
        .drain(..)
        .map(|contact| (contact.contact_id.clone(), contact))
        .collect::<BTreeMap<_, _>>();
    for record in state.contacts.values() {
        if let Some(contact) = home_people_contact_summary_from_record(record) {
            contacts.insert(contact.contact_id.clone(), contact);
        }
    }
    people.contacts = contacts.into_values().collect();
    people.contacts.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.contact_id.cmp(&right.contact_id))
    });
    people.contact_count = people.contacts.len();
    let non_contact_offers = people
        .service_offers
        .iter()
        .filter(|offer| offer.contact_id.is_none())
        .cloned()
        .collect::<Vec<_>>();
    people.service_offers = people
        .contacts
        .iter()
        .flat_map(home_service_offers_for_people_contact)
        .chain(non_contact_offers)
        .collect();
    people.service_offer_count = people.service_offers.len();
    services.remote_offers = people.service_offers.clone();
    services
        .remote_offers
        .extend(home_configured_remote_exit_offers(data_dir));
    services.remote_offers.sort_by(|left, right| {
        left.service_kind
            .cmp(&right.service_kind)
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.offer_id.cmp(&right.offer_id))
    });
    services.remote_offer_count = services.remote_offers.len();
    Ok(())
}

fn default_home_people_removed_contacts(
    context: &HomeLaunchTokenContext,
) -> HomePeopleRemovedContacts {
    HomePeopleRemovedContacts {
        schema: HOME_PEOPLE_REMOVED_CONTACTS_SCHEMA.to_string(),
        principal_id: home_browser_principal_id(context),
        localhost_root: home_browser_localhost_root(context),
        contacts: BTreeMap::new(),
    }
}

fn home_people_removed_contacts_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_people_removed_contacts_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid People state root"))
}

fn home_people_removed_contacts_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/people-removed-contacts.json",
        home_browser_localhost_root(context)
    )
}

fn home_people_removed_contacts(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomePeopleRemovedContacts> {
    let path = home_people_removed_contacts_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(default_home_people_removed_contacts(context));
    }
    let principal_id = home_browser_principal_id(context);
    let localhost_root = home_browser_localhost_root(context);
    let bytes = match crate::auth::read_principal_root_object(
        data_dir,
        &principal_id,
        &localhost_root,
        &home_people_removed_contacts_uri(context),
        &path,
    ) {
        Ok(bytes) => bytes,
        Err(err) if is_unencrypted_principal_root_state(&err) => {
            return Ok(default_home_people_removed_contacts(context));
        }
        Err(err) if is_missing_principal_root_state_file(&err) => {
            return Ok(default_home_people_removed_contacts(context));
        }
        Err(err) => return Err(err),
    };
    let state: HomePeopleRemovedContacts = serde_json::from_slice(&bytes)?;
    if state.schema != HOME_PEOPLE_REMOVED_CONTACTS_SCHEMA {
        anyhow::bail!("unsupported People removed contacts schema");
    }
    if state.principal_id != principal_id {
        anyhow::bail!("People removed contacts principal mismatch");
    }
    if state.localhost_root != localhost_root {
        anyhow::bail!("People removed contacts root mismatch");
    }
    Ok(state)
}

fn home_save_people_removed_contacts(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &HomePeopleRemovedContacts,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if bytes.len() > HOME_PEOPLE_REMOVED_CONTACTS_MAX_BYTES {
        anyhow::bail!("People removed contacts state is too large");
    }
    let path = home_people_removed_contacts_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_people_removed_contacts_uri(context),
        &path,
        &bytes,
    )?;
    Ok(())
}

fn home_mark_people_contact_removed(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    contact_id: &str,
    removed_at: u64,
) -> anyhow::Result<()> {
    let mut state = home_people_removed_contacts(data_dir, context)?;
    state.contacts.insert(contact_id.to_string(), removed_at);
    home_save_people_removed_contacts(data_dir, context, &state)
}

fn default_home_people_discovery_state(
    context: &HomeLaunchTokenContext,
) -> HomePeopleDiscoveryState {
    HomePeopleDiscoveryState {
        schema: HOME_PEOPLE_DISCOVERY_STATE_SCHEMA.to_string(),
        principal_id: home_browser_principal_id(context),
        localhost_root: home_browser_localhost_root(context),
        enabled: false,
        enabled_until: None,
        updated_at: 0,
        local_peer_id: None,
        last_bootstrap_at: None,
        last_presence_sent_at: None,
        peers: BTreeMap::new(),
        requests: BTreeMap::new(),
    }
}

fn home_people_discovery_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/people-discovery.json",
        home_browser_localhost_root(context)
    )
}

fn home_people_discovery_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_people_discovery_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid People discovery state root"))
}

fn home_people_discovery_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomePeopleDiscoveryState> {
    let path = home_people_discovery_path(data_dir, context)?;
    if !path.is_file() {
        return Ok(default_home_people_discovery_state(context));
    }
    let principal_id = home_browser_principal_id(context);
    let localhost_root = home_browser_localhost_root(context);
    let bytes = match crate::auth::read_principal_root_object(
        data_dir,
        &principal_id,
        &localhost_root,
        &home_people_discovery_uri(context),
        &path,
    ) {
        Ok(bytes) => bytes,
        Err(err) if is_unencrypted_principal_root_state(&err) => {
            return Ok(default_home_people_discovery_state(context));
        }
        Err(err) if is_missing_principal_root_state_file(&err) => {
            return Ok(default_home_people_discovery_state(context));
        }
        Err(err) => return Err(err),
    };
    let state: HomePeopleDiscoveryState = serde_json::from_slice(&bytes)?;
    if state.schema != HOME_PEOPLE_DISCOVERY_STATE_SCHEMA {
        anyhow::bail!("unsupported People discovery state schema");
    }
    if state.principal_id != principal_id {
        anyhow::bail!("People discovery principal mismatch");
    }
    if state.localhost_root != localhost_root {
        anyhow::bail!("People discovery root mismatch");
    }
    Ok(state)
}

fn home_save_people_discovery_state(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &HomePeopleDiscoveryState,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    if bytes.len() > HOME_PEOPLE_DISCOVERY_MAX_BYTES {
        anyhow::bail!("People discovery state is too large");
    }
    let path = home_people_discovery_path(data_dir, context)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_people_discovery_uri(context),
        &path,
        &bytes,
    )?;
    Ok(())
}

fn home_people_discovery_summary(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomePeopleDiscoverySummary> {
    let state = home_people_discovery_state(data_dir, context)?;
    let now = now_ts();
    let enabled = home_people_discovery_active(&state, now);
    let expires_at = enabled.then_some(state.enabled_until.unwrap_or(now));
    let remaining_seconds = expires_at.map(|expires_at| expires_at.saturating_sub(now));
    let local_peer_id = enabled.then_some(state.local_peer_id.clone()).flatten();
    let contacts = home_people_contacts_state(data_dir, context)?;
    let removed_contacts = home_people_removed_contacts(data_dir, context)?;
    let mut active_contact_peer_ids = BTreeSet::new();
    let mut active_contact_dids = BTreeSet::new();
    for contact in contacts.contacts.values() {
        let removed_at = removed_contacts.contacts.get(&contact.contact_id).copied();
        let active = removed_at
            .map(|removed_at| contact.added_at > removed_at)
            .unwrap_or(true);
        if !active {
            continue;
        }
        let peer_id = contact.peer_id.trim();
        if !peer_id.is_empty() {
            active_contact_peer_ids.insert(peer_id.to_string());
        }
        if let Some(did) = contact
            .did
            .as_deref()
            .map(str::trim)
            .filter(|did| !did.is_empty())
        {
            active_contact_dids.insert(did.to_string());
        }
    }
    let status = if !enabled {
        "off"
    } else if local_peer_id.is_some() {
        "visible"
    } else {
        "runtime_unavailable"
    };
    let status_message = match status {
        "off" => "Discovery is off.".to_string(),
        "visible" => "Discovery is on. Other ElastOS homes can request to add you when peer discovery is running.".to_string(),
        _ => "Discovery is on, but local discovery is not reachable yet.".to_string(),
    };
    let mut discovered_peers = state
        .peers
        .into_values()
        .filter(|peer| {
            !active_contact_peer_ids.contains(peer.peer_id.trim())
                && peer
                    .did
                    .as_deref()
                    .map(str::trim)
                    .filter(|did| !did.is_empty())
                    .map(|did| !active_contact_dids.contains(did))
                    .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    discovered_peers.sort_by(|left, right| {
        right
            .last_seen_at
            .cmp(&left.last_seen_at)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    let mut requests = state
        .requests
        .into_values()
        .filter(home_people_discovery_request_visible)
        .collect::<Vec<_>>();
    requests.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(HomePeopleDiscoverySummary {
        schema: HOME_PEOPLE_DISCOVERY_SCHEMA.to_string(),
        enabled,
        expires_at,
        remaining_seconds,
        visibility: if enabled {
            "everyone".to_string()
        } else {
            "off".to_string()
        },
        status: status.to_string(),
        status_message,
        topic: HOME_PEOPLE_DISCOVERY_TOPIC.to_string(),
        local_peer_id,
        discovered_count: discovered_peers.len(),
        request_count: requests.len(),
        discovered_peers,
        requests,
        changed: None,
        refresh_fingerprint: None,
        next_refresh_after_ms: None,
    })
}

fn home_people_discovery_summary_signature(summary: &HomePeopleDiscoverySummary) -> String {
    let value = serde_json::json!({
        "enabled": summary.enabled,
        "status": summary.status,
        "local_peer_id": summary.local_peer_id,
        "discovered_peers": summary.discovered_peers,
        "requests": summary.requests,
    });
    stable_cursor_hash(&value)
}

fn home_people_discovery_state_signature(state: &HomePeopleDiscoveryState) -> String {
    let requests = state
        .requests
        .values()
        .filter(|request| home_people_discovery_request_visible(request))
        .collect::<Vec<_>>();
    let value = serde_json::json!({
        "enabled": state.enabled,
        "local_peer_id": state.local_peer_id,
        "peers": state.peers,
        "requests": requests,
    });
    stable_cursor_hash(&value)
}

fn home_people_discovery_next_refresh_after_ms(
    summary: &HomePeopleDiscoverySummary,
    changed: bool,
) -> u64 {
    if !summary.enabled {
        return 0;
    }
    if changed {
        return HOME_PEOPLE_DISCOVERY_REFRESH_FAST_MS;
    }
    if summary.discovered_count == 0 && summary.request_count == 0 {
        return HOME_PEOPLE_DISCOVERY_REFRESH_SEARCH_MS;
    }
    HOME_PEOPLE_DISCOVERY_REFRESH_IDLE_MS
}

fn home_people_discovery_annotate_refresh(summary: &mut HomePeopleDiscoverySummary, changed: bool) {
    summary.changed = Some(changed);
    summary.refresh_fingerprint = Some(home_people_discovery_summary_signature(summary));
    summary.next_refresh_after_ms = Some(home_people_discovery_next_refresh_after_ms(
        summary, changed,
    ));
}

fn home_people_discovery_peers_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("config/people-discovery-peers.json")
}

fn home_people_discovery_configured_tickets(
    data_dir: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    let path = home_people_discovery_peers_path(data_dir);
    let Ok(raw) = std::fs::read(&path) else {
        return Ok(Vec::new());
    };
    if raw.len() > HOME_PEOPLE_DISCOVERY_PEERS_MAX_BYTES {
        anyhow::bail!("People discovery peers config is too large");
    }
    let config: HomePeopleDiscoveryPeersConfig =
        serde_json::from_slice(&raw).with_context(|| {
            format!(
                "invalid People discovery peers config at {}",
                path.display()
            )
        })?;
    if !config.schema.trim().is_empty() && config.schema != HOME_PEOPLE_DISCOVERY_PEERS_SCHEMA {
        anyhow::bail!("unsupported People discovery peers config schema");
    }
    let mut seen = BTreeSet::new();
    Ok(config
        .peers
        .into_iter()
        .filter_map(|peer| {
            let ticket = peer.connect_ticket.trim().to_string();
            if ticket.is_empty() || ticket.len() > 8192 || !seen.insert(ticket.clone()) {
                return None;
            }
            Some(ticket)
        })
        .collect())
}

fn home_people_discovery_connect_configured_peers(
    runtime: &PeoplePeerRuntimeBlocking,
    data_dir: &std::path::Path,
) -> Vec<String> {
    let mut peers = BTreeSet::new();
    let tickets = match home_people_discovery_configured_tickets(data_dir) {
        Ok(tickets) => tickets,
        Err(err) => {
            tracing::warn!("people discovery peer bootstrap config ignored: {err}");
            return Vec::new();
        }
    };
    for ticket in tickets {
        let response = match people_peer_provider_request_blocking(
            &runtime.client,
            &runtime.api_url,
            &runtime.client_token,
            &runtime.peer_cap,
            "connect",
            serde_json::json!({ "ticket": ticket }),
        ) {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!("people discovery configured peer connect failed: {err}");
                continue;
            }
        };
        for field in ["added", "connected"] {
            if let Some(values) = response
                .get("data")
                .and_then(|data| data.get(field))
                .and_then(serde_json::Value::as_array)
            {
                for value in values {
                    if let Some(peer) = value
                        .as_str()
                        .map(str::trim)
                        .filter(|peer| !peer.is_empty())
                    {
                        peers.insert(peer.to_string());
                    }
                }
            }
        }
    }
    peers.into_iter().collect()
}

fn home_people_discovery_join_known_peers(
    runtime: &PeoplePeerRuntimeBlocking,
    topic: &str,
    peers: &[String],
) -> anyhow::Result<()> {
    if peers.is_empty() {
        return Ok(());
    }
    people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_join_peers",
        serde_json::json!({ "topic": topic, "peers": peers }),
    )
    .map(|_| ())
}

fn home_people_discovery_sync(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &mut HomePeopleDiscoveryState,
) -> anyhow::Result<()> {
    let now = now_ts();
    if home_people_discovery_apply_expiry(state, now) {
        return Ok(());
    }
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    let local_peer_changed = state.local_peer_id.as_deref() != Some(runtime.peer_id.as_str());
    let bootstrap_due = local_peer_changed
        || state
            .last_bootstrap_at
            .map(|last| now.saturating_sub(last) >= HOME_PEOPLE_DISCOVERY_BOOTSTRAP_INTERVAL_SECS)
            .unwrap_or(true);
    if bootstrap_due {
        let configured_peers = home_people_discovery_connect_configured_peers(&runtime, data_dir);
        people_peer_gossip_join_blocking(&runtime, HOME_PEOPLE_DISCOVERY_TOPIC, "dht")?;
        if let Err(err) = home_people_discovery_join_known_peers(
            &runtime,
            HOME_PEOPLE_DISCOVERY_TOPIC,
            &configured_peers,
        ) {
            tracing::warn!("people discovery configured peer join failed: {err}");
        }
        state.last_bootstrap_at = Some(now);
    }
    let profile = home_profile_card_summary_for_context(data_dir, context);
    let local_did = home_people_local_did(data_dir).ok();
    let display_name = profile
        .as_ref()
        .map(|profile| profile.display_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ElastOS user".to_string());
    let handle = profile.as_ref().and_then(|profile| profile.handle.clone());
    let presence = serde_json::json!({
        "schema": "elastos.people.discovery-presence/v1",
        "kind": "presence",
        "peer_id": &runtime.peer_id,
        "did": local_did,
        "display_name": display_name,
        "handle": handle,
        "updated_at": now,
        "expires_at": now + 180,
    });
    let presence_due = local_peer_changed
        || state
            .last_presence_sent_at
            .map(|last| now.saturating_sub(last) >= HOME_PEOPLE_DISCOVERY_PRESENCE_INTERVAL_SECS)
            .unwrap_or(true);
    if presence_due {
        let _ = people_peer_provider_request_blocking(
            &runtime.client,
            &runtime.api_url,
            &runtime.client_token,
            &runtime.peer_cap,
            "gossip_send",
            serde_json::json!({
                "topic": HOME_PEOPLE_DISCOVERY_TOPIC,
                "sender_id": &runtime.peer_id,
                "sender": display_name,
                "message": presence.to_string(),
                "ts": now,
            }),
        )?;
        state.last_presence_sent_at = Some(now);
    }
    let recv = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": HOME_PEOPLE_DISCOVERY_TOPIC,
            "consumer_id": format!("home-people-discovery:{}", context.principal_id),
            "skip_sender_id": runtime.peer_id,
            "limit": 64,
        }),
    )?;
    let messages = recv
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for message in messages {
        let Some(content) = message.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        match payload.get("kind").and_then(serde_json::Value::as_str) {
            Some("presence") => merge_people_discovery_presence(state, &payload),
            Some("request") => merge_people_discovery_request(state, &payload, &runtime.peer_id),
            Some("invite") => {
                let _ = merge_people_discovery_invite(data_dir, state, &payload, &runtime.peer_id);
            }
            Some("acceptance") => {
                let _ = merge_people_discovery_acceptance(
                    data_dir,
                    context,
                    state,
                    &payload,
                    &runtime.peer_id,
                );
            }
            _ => {}
        }
    }
    state.local_peer_id = Some(runtime.peer_id);
    home_people_discovery_sync_contacts(data_dir, context, state)?;
    prune_people_discovery_state(state, now);
    Ok(())
}

fn home_people_discovery_send_request(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &mut HomePeopleDiscoveryState,
    request: &HomePeopleDiscoveryRequestSummary,
) -> anyhow::Result<()> {
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_PEOPLE_DISCOVERY_TOPIC, "dht")?;
    let requester_did = home_people_local_did(data_dir)?;
    let profile = home_profile_card_summary_for_context(data_dir, context);
    let display_name = profile
        .as_ref()
        .map(|profile| profile.display_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ElastOS user".to_string());
    let handle = profile.as_ref().and_then(|profile| profile.handle.clone());
    let payload = serde_json::json!({
        "schema": "elastos.people.discovery-request/v1",
        "kind": "request",
        "request_id": &request.request_id,
        "peer_id": &runtime.peer_id,
        "requester_did": requester_did,
        "target_peer_id": &request.peer_id,
        "display_name": &display_name,
        "handle": handle,
        "created_at": request.created_at,
    });
    let _ = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": HOME_PEOPLE_DISCOVERY_TOPIC,
            "sender_id": &runtime.peer_id,
            "sender": display_name,
            "message": payload.to_string(),
            "ts": request.created_at,
        }),
    )?;
    state.local_peer_id = Some(runtime.peer_id);
    Ok(())
}

fn home_people_discovery_send_acceptance(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &mut HomePeopleDiscoveryState,
    request: &HomePeopleDiscoveryRequestSummary,
) -> anyhow::Result<()> {
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_PEOPLE_DISCOVERY_TOPIC, "dht")?;
    let accepter_did = home_people_local_did(data_dir)?;
    let profile = home_profile_card_summary_for_context(data_dir, context);
    let display_name = profile
        .as_ref()
        .map(|profile| profile.display_name.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ElastOS user".to_string());
    let handle = profile.as_ref().and_then(|profile| profile.handle.clone());
    let now = now_ts();
    let payload = serde_json::json!({
        "schema": "elastos.people.discovery-acceptance/v1",
        "kind": "acceptance",
        "request_id": &request.request_id,
        "peer_id": &runtime.peer_id,
        "target_peer_id": &request.peer_id,
        "accepter_did": accepter_did,
        "display_name": &display_name,
        "handle": handle,
        "created_at": now,
    });
    let _ = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": HOME_PEOPLE_DISCOVERY_TOPIC,
            "sender_id": &runtime.peer_id,
            "sender": display_name,
            "message": payload.to_string(),
            "ts": now,
        }),
    )?;
    state.local_peer_id = Some(runtime.peer_id);
    Ok(())
}

fn home_people_discovery_send_room_acceptance(
    data_dir: &std::path::Path,
    state: &mut HomePeopleDiscoveryState,
    request: &HomePeopleDiscoveryRequestSummary,
    acceptance: &crate::room_service::SignedRoomAcceptEnvelope,
) -> anyhow::Result<()> {
    let runtime = people_attach_peer_runtime_blocking(data_dir)?;
    people_peer_gossip_join_blocking(&runtime, HOME_PEOPLE_DISCOVERY_TOPIC, "dht")?;
    let now = now_ts();
    let sender =
        clean_people_person_display_name(Some(&request.display_name), request.handle.as_deref());
    let payload = serde_json::json!({
        "schema": "elastos.people.discovery-acceptance/v1",
        "kind": "acceptance",
        "request_id": &request.request_id,
        "peer_id": &runtime.peer_id,
        "target_peer_id": &request.peer_id,
        "acceptance": acceptance,
        "created_at": now,
    });
    let _ = people_peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": HOME_PEOPLE_DISCOVERY_TOPIC,
            "sender_id": &runtime.peer_id,
            "sender": sender,
            "message": payload.to_string(),
            "ts": now,
        }),
    )?;
    state.local_peer_id = Some(runtime.peer_id);
    Ok(())
}

fn merge_people_discovery_presence(
    state: &mut HomePeopleDiscoveryState,
    payload: &serde_json::Value,
) {
    let peer_id = payload
        .get("peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    if peer_id.is_empty() || Some(peer_id) == state.local_peer_id.as_deref() {
        return;
    }
    let last_seen_at = payload
        .get("updated_at")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(now_ts);
    let handle = clean_people_payload_handle(payload);
    let display_name = clean_people_payload_display_name(payload, handle.as_deref(), None);
    state.peers.insert(
        peer_id.to_string(),
        HomePeopleDiscoveryPeerSummary {
            peer_id: peer_id.to_string(),
            did: payload
                .get("did")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            display_name,
            handle,
            last_seen_at,
            status: "visible".to_string(),
        },
    );
}

fn merge_people_discovery_request(
    state: &mut HomePeopleDiscoveryState,
    payload: &serde_json::Value,
    local_peer_id: &str,
) {
    if payload
        .get("target_peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some(local_peer_id)
    {
        return;
    }
    let request_id = payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    let peer_id = payload
        .get("peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("");
    if request_id.is_empty() || peer_id.is_empty() {
        return;
    }
    if let Some(existing) = state.requests.get_mut(request_id) {
        let handle = clean_people_payload_handle(payload).or_else(|| existing.handle.clone());
        let fallback_display_name = existing.display_name.clone();
        existing.peer_id = peer_id.to_string();
        existing.did = payload
            .get("requester_did")
            .or_else(|| payload.get("did"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| existing.did.clone());
        existing.display_name = clean_people_payload_display_name(
            payload,
            handle.as_deref(),
            Some(fallback_display_name.as_str()),
        );
        existing.handle = handle;
        if existing.status != "joined" && existing.status != "accepted_sent" {
            existing.status = "incoming".to_string();
            existing.invite_id = None;
        }
        return;
    }
    let handle = clean_people_payload_handle(payload);
    let display_name = clean_people_payload_display_name(payload, handle.as_deref(), None);
    state.requests.insert(
        request_id.to_string(),
        HomePeopleDiscoveryRequestSummary {
            request_id: request_id.to_string(),
            peer_id: peer_id.to_string(),
            did: payload
                .get("requester_did")
                .or_else(|| payload.get("did"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            display_name,
            handle,
            created_at: payload
                .get("created_at")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_else(now_ts),
            status: "incoming".to_string(),
            invite_id: None,
        },
    );
}

fn merge_people_discovery_invite(
    data_dir: &std::path::Path,
    state: &mut HomePeopleDiscoveryState,
    payload: &serde_json::Value,
    local_peer_id: &str,
) -> anyhow::Result<()> {
    if payload
        .get("target_peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some(local_peer_id)
    {
        return Ok(());
    }
    let request_id = payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("discovery invite is missing a request id"))?;
    let peer_id = payload
        .get("peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("discovery invite is missing a peer id"))?;
    let invite_value = payload
        .get("invite")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("discovery invite is missing a signed invite"))?;
    let invite_bytes = serde_json::to_vec(&invite_value)?;
    let imported = crate::room_service::import_room_invite_envelope(data_dir, &invite_bytes)?;
    let handle = clean_people_payload_handle(payload);
    let display_name = clean_people_payload_display_name(payload, handle.as_deref(), None);
    let now = now_ts();
    let entry = state
        .requests
        .entry(request_id.to_string())
        .or_insert_with(|| HomePeopleDiscoveryRequestSummary {
            request_id: request_id.to_string(),
            peer_id: peer_id.to_string(),
            did: Some(imported.invited_by.clone()),
            display_name: display_name.clone(),
            handle: handle.clone(),
            created_at: now,
            status: "accepted".to_string(),
            invite_id: Some(imported.invite_id.clone()),
        });
    entry.peer_id = peer_id.to_string();
    entry.did = Some(imported.invited_by);
    entry.display_name = display_name;
    entry.handle = handle;
    entry.status = "accepted".to_string();
    entry.invite_id = Some(imported.invite_id);
    state.updated_at = now;
    Ok(())
}

fn merge_people_discovery_acceptance(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &mut HomePeopleDiscoveryState,
    payload: &serde_json::Value,
    local_peer_id: &str,
) -> anyhow::Result<()> {
    if payload
        .get("target_peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some(local_peer_id)
    {
        return Ok(());
    }
    let request_id = payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("discovery acceptance is missing a request id"))?;
    let peer_id = payload
        .get("peer_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("discovery acceptance is missing a peer id"))?;
    let mut did = payload
        .get("accepter_did")
        .or_else(|| payload.get("did"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let mut imported_room_acceptance = false;
    if did.is_none() {
        if let Some(acceptance_value) = payload.get("acceptance").cloned() {
            let acceptance_bytes = serde_json::to_vec(&acceptance_value)?;
            let member =
                crate::room_service::import_room_acceptance_envelope(data_dir, &acceptance_bytes)?;
            did = Some(member.member_did);
            imported_room_acceptance = true;
        }
    }
    let did =
        did.ok_or_else(|| anyhow::anyhow!("discovery acceptance is missing an accepter DID"))?;
    let fallback_display_name = state
        .requests
        .get(request_id)
        .map(|request| request.display_name.clone());
    let handle = clean_people_payload_handle(payload).or_else(|| {
        state
            .requests
            .get(request_id)
            .and_then(|request| request.handle.clone())
    });
    let display_name = clean_people_payload_display_name(
        payload,
        handle.as_deref(),
        fallback_display_name.as_deref(),
    );
    let now = now_ts();
    let _ = home_people_upsert_contact(
        data_dir,
        context,
        HomePeopleContactUpsert {
            peer_id,
            did: &did,
            display_name: &display_name,
            handle: handle.as_deref(),
            source: "people_discovery",
            updated_at: now,
        },
    )?;
    let entry = state
        .requests
        .entry(request_id.to_string())
        .or_insert_with(|| HomePeopleDiscoveryRequestSummary {
            request_id: request_id.to_string(),
            peer_id: peer_id.to_string(),
            did: Some(did.clone()),
            display_name: display_name.clone(),
            handle: handle.clone(),
            created_at: now,
            status: "joined".to_string(),
            invite_id: None,
        });
    entry.peer_id = peer_id.to_string();
    entry.did = Some(did);
    entry.display_name = display_name;
    entry.handle = handle;
    entry.status = "joined".to_string();
    entry.invite_id = None;
    state.updated_at = now;
    if imported_room_acceptance {
        let summary = crate::room_service::load_summary(data_dir).unwrap_or_default();
        let _ = crate::notifications::sync_room_notifications(data_dir, &summary);
    }
    Ok(())
}

fn home_people_discovery_sync_contacts(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    state: &mut HomePeopleDiscoveryState,
) -> anyhow::Result<()> {
    let requests = state.requests.values().cloned().collect::<Vec<_>>();
    for request in requests {
        if request.status != "joined"
            && request.status != "accepted_sent"
            && request.status != "accepted"
        {
            continue;
        }
        let Some(did) = request
            .did
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let now = now_ts();
        let (_, newly_visible) = home_people_upsert_contact(
            data_dir,
            context,
            HomePeopleContactUpsert {
                peer_id: &request.peer_id,
                did,
                display_name: &request.display_name,
                handle: request.handle.as_deref(),
                source: "people_discovery",
                updated_at: now,
            },
        )?;
        if let Some(stored) = state.requests.get_mut(&request.request_id) {
            stored.status = "joined".to_string();
            stored.invite_id = None;
        }
        if newly_visible {
            let _ = home_people_discovery_send_acceptance(data_dir, context, state, &request);
        }
    }
    Ok(())
}

fn prune_people_discovery_state(state: &mut HomePeopleDiscoveryState, now: u64) {
    let _ = home_people_discovery_apply_expiry(state, now);
    state
        .peers
        .retain(|_, peer| now.saturating_sub(peer.last_seen_at) <= 300);
    while state.requests.len() > 64 {
        let Some(oldest) = state
            .requests
            .iter()
            .min_by_key(|(_, request)| request.created_at)
            .map(|(request_id, _)| request_id.clone())
        else {
            break;
        };
        state.requests.remove(&oldest);
    }
}

fn home_people_discovery_active(state: &HomePeopleDiscoveryState, now: u64) -> bool {
    state.enabled
        && state
            .enabled_until
            .map(|enabled_until| enabled_until > now)
            .unwrap_or(false)
}

fn home_people_discovery_apply_expiry(state: &mut HomePeopleDiscoveryState, now: u64) -> bool {
    if state.enabled && !home_people_discovery_active(state, now) {
        state.enabled = false;
        state.enabled_until = None;
        state.local_peer_id = None;
        state.last_bootstrap_at = None;
        state.last_presence_sent_at = None;
        state.updated_at = now;
        return true;
    }
    false
}

fn home_people_local_did(data_dir: &std::path::Path) -> anyhow::Result<String> {
    let (_, did) = elastos_identity::load_or_create_did(data_dir)?;
    let did = did.trim();
    if did.is_empty() {
        anyhow::bail!("local ElastOS DID is empty");
    }
    Ok(did.to_string())
}

fn home_people_discovery_request_id(peer_id: &str) -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|err| anyhow::anyhow!("request id rng: {err}"))?;
    Ok(format!("request:{peer_id}:{}", hex::encode(bytes)))
}

fn people_attach_peer_runtime_blocking(
    data_dir: &std::path::Path,
) -> anyhow::Result<PeoplePeerRuntimeBlocking> {
    let coords = load_runtime_coords(data_dir)
        .ok_or_else(|| anyhow::anyhow!("local ElastOS service is not running"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let token = people_attach_client_token_blocking(&client, &coords)
        .ok_or_else(|| anyhow::anyhow!("failed to attach to local ElastOS service"))?;
    let cap = people_request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &token,
        "elastos://peer/*",
        "message",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire Carrier peer capability"))?;
    let response = people_peer_provider_request_blocking(
        &client,
        &coords.api_url,
        &token,
        &cap,
        "get_ticket",
        serde_json::json!({}),
    )?;
    let peer_id = response
        .get("data")
        .and_then(|data| data.get("node_id"))
        .and_then(serde_json::Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Carrier peer provider did not return a node id"))?;
    let connect_ticket = response
        .get("data")
        .and_then(|data| data.get("ticket"))
        .and_then(serde_json::Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| value.len() <= HOME_SERVICES_REMOTE_EXIT_TICKET_MAX_BYTES)
        .ok_or_else(|| anyhow::anyhow!("Carrier peer provider did not return a bounded ticket"))?;
    Ok(PeoplePeerRuntimeBlocking {
        client,
        api_url: coords.api_url,
        client_token: token,
        peer_cap: cap,
        peer_id,
        connect_ticket,
    })
}

fn people_peer_gossip_join_blocking(
    runtime: &PeoplePeerRuntimeBlocking,
    topic: &str,
    mode: &str,
) -> anyhow::Result<()> {
    match people_peer_provider_request_blocking(
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

fn people_attach_client_token_blocking(
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

fn people_request_attached_capability_blocking(
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
    if let Some(token) = body.get("token").and_then(|token| token.as_str()) {
        return Some(token.to_string());
    }
    let request_id = body
        .get("request_id")
        .and_then(|request_id| request_id.as_str())?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let status: serde_json::Value = client
            .get(format!("{}/api/capability/request/{}", api, request_id))
            .header("Authorization", format!("Bearer {}", client_token))
            .send()
            .ok()?
            .json()
            .ok()?;
        if let Some(token) = status.get("token").and_then(|token| token.as_str()) {
            return Some(token.to_string());
        }
        match status.get("status").and_then(|status| status.as_str()) {
            Some("denied") | Some("expired") => return None,
            _ => {}
        }
    }
    None
}

fn people_peer_provider_request_blocking(
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

fn filter_removed_people_contacts(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    people: &mut HomePeopleSummary,
    services: &mut HomeServicesSummary,
) -> anyhow::Result<()> {
    let removed = home_people_removed_contacts(data_dir, context)?;
    if removed.contacts.is_empty() {
        return Ok(());
    }
    people
        .contacts
        .retain(|contact| match removed.contacts.get(&contact.contact_id) {
            Some(removed_at) => contact.added_at > *removed_at,
            None => true,
        });
    people.contact_count = people.contacts.len();
    let live_contact_ids = people
        .contacts
        .iter()
        .map(|contact| contact.contact_id.as_str())
        .collect::<BTreeSet<_>>();
    people.service_offers.retain(|offer| {
        offer
            .contact_id
            .as_deref()
            .map(|contact_id| live_contact_ids.contains(contact_id))
            .unwrap_or(true)
    });
    people.service_offer_count = people.service_offers.len();
    services.remote_offers.retain(|offer| {
        offer
            .contact_id
            .as_deref()
            .map(|contact_id| live_contact_ids.contains(contact_id))
            .unwrap_or(true)
    });
    services.remote_offer_count = services.remote_offers.len();
    services.available_remote_offers.retain(|offer| {
        offer
            .contact_id
            .as_deref()
            .map(|contact_id| live_contact_ids.contains(contact_id))
            .unwrap_or(true)
    });
    services.available_remote_offer_count = services.available_remote_offers.len();
    Ok(())
}

fn sanitize_recent_targets(targets: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for target in targets {
        let value = target.trim();
        if value.is_empty()
            || value.len() > 64
            || !value
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
            || !seen.insert(value.to_string())
        {
            continue;
        }
        out.push(value.to_string());
        if out.len() >= 10 {
            break;
        }
    }
    out
}

fn sanitize_home_browser_state_targets(
    data_dir: &std::path::Path,
    state: &mut HomeBrowserStateSummary,
) {
    let known_targets = home_targets(data_dir)
        .into_iter()
        .map(|target| target.target)
        .collect::<BTreeSet<_>>();
    state
        .recent_targets
        .retain(|target| known_targets.contains(target));
    let localhost_root = state.localhost_root.clone();
    state.layout = state
        .layout
        .take()
        .and_then(|layout| sanitize_home_layout_targets(layout, &known_targets, &localhost_root));
    state.session = state
        .session
        .take()
        .and_then(|session| sanitize_home_session_targets(session, &known_targets));
}

fn sanitize_home_layout_targets(
    mut layout: serde_json::Value,
    known_targets: &BTreeSet<String>,
    localhost_root: &str,
) -> Option<serde_json::Value> {
    let layout_object = layout.as_object_mut()?;
    if let Some(desktop) = layout_object
        .get_mut("desktop")
        .and_then(|value| value.as_object_mut())
    {
        desktop.retain(|entry, _position| {
            known_targets.contains(entry)
                || is_home_desktop_object_layout_entry(localhost_root, entry)
        });
    }
    if let Some(labels) = layout_object
        .get_mut("desktopLabels")
        .and_then(|value| value.as_object_mut())
    {
        labels.retain(|target, _label| known_targets.contains(target));
    }
    if let Some(hidden) = layout_object.get_mut("desktopHidden") {
        *hidden = sanitize_home_target_array(hidden.take(), known_targets);
    }
    if let Some(taskbar) = layout_object.get_mut("taskbar") {
        *taskbar = sanitize_home_target_array(taskbar.take(), known_targets);
    }
    Some(layout)
}

fn is_home_desktop_object_layout_entry(localhost_root: &str, entry: &str) -> bool {
    let Some(uri) = entry.strip_prefix("object:") else {
        return false;
    };
    if uri.len() > 2048 || uri.contains('\0') {
        return false;
    }
    let root = localhost_root.trim_end_matches('/');
    if root.is_empty() {
        return false;
    }
    uri == format!("{root}/.Trash") || uri.starts_with(&format!("{root}/Desktop/"))
}

fn sanitize_home_session_targets(
    mut session: serde_json::Value,
    known_targets: &BTreeSet<String>,
) -> Option<serde_json::Value> {
    let session_object = session.as_object_mut()?;
    let windows = session_object
        .get_mut("windows")
        .and_then(|value| value.as_array_mut())?;
    windows.retain(|window| {
        window
            .get("target")
            .and_then(|target| target.as_str())
            .is_some_and(|target| known_targets.contains(target))
    });
    if windows.is_empty() {
        return None;
    }
    Some(session)
}

fn sanitize_home_target_array(
    value: serde_json::Value,
    known_targets: &BTreeSet<String>,
) -> serde_json::Value {
    let mut seen = BTreeSet::new();
    let targets = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|target| target.as_str())
        .filter(|target| known_targets.contains(*target) && seen.insert((*target).to_string()))
        .map(|target| serde_json::Value::String(target.to_string()))
        .collect::<Vec<_>>();
    serde_json::Value::Array(targets)
}

pub(super) async fn home_runtime_ensure(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_token(&state.data_dir, &headers) {
        return home_error_response(err);
    }

    Json(ensure_home_runtime(&state.data_dir).await).into_response()
}

pub(super) async fn system_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[SYSTEM_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return system_error_response(err),
        };
    let context = authority.home_launch_context();

    let (runtime, wallet_accounts, wallet_approvals, runtime_log) = tokio::join!(
        home_runtime_summary(&state.data_dir),
        system_wallet_accounts_summary(&state, &authority),
        system_wallet_approvals_summary(&state, &authority, false),
        system_runtime_log(&state.data_dir)
    );
    Json(SystemSummaryResponse {
        identity: load_gateway_identity_summary_for_context(&state.data_dir, &context),
        authority: home_authority_summary(&context),
        access: system_access_summary(&state.data_dir, &context),
        home: HomeCapsuleIdentity {
            id: HOME_CAPSULE_ID.to_string(),
            route: HOME_ROUTE.to_string(),
        },
        app: SystemCapsuleIdentity {
            id: SYSTEM_CAPSULE_ID.to_string(),
            route: SYSTEM_ROUTE.to_string(),
        },
        appearance: match home_appearance_summary(&state.data_dir, &context) {
            Ok(appearance) => appearance,
            Err(err) => return system_error_response(err),
        },
        source: system_source_summary(&state.data_dir, &runtime),
        runtime,
        wallet_accounts,
        wallet_approvals,
        runtime_log,
    })
    .into_response()
}

fn system_source_summary(
    data_dir: &std::path::Path,
    runtime: &HomeRuntimeSummary,
) -> SystemSourceSummary {
    let runtime_version = runtime
        .version
        .as_deref()
        .unwrap_or(GATEWAY_VERSION)
        .to_string();
    let config = match crate::sources::load_trusted_sources(data_dir) {
        Ok(config) => config,
        Err(err) => {
            return SystemSourceSummary {
                configured: false,
                name: None,
                channel: "unknown".to_string(),
                installed_version: "unknown".to_string(),
                runtime_version,
                mode: "development".to_string(),
                update_checks_allowed: false,
                update_policy: format!("trusted source configuration could not be read: {err}"),
                transport: "Carrier trusted source unavailable".to_string(),
                source_peer: None,
            };
        }
    };
    let Some(source) = config.default_source() else {
        return SystemSourceSummary {
            configured: false,
            name: None,
            channel: "not configured".to_string(),
            installed_version: "unknown".to_string(),
            runtime_version,
            mode: "development".to_string(),
            update_checks_allowed: false,
            update_policy: "No trusted source configured. Add one before checking updates."
                .to_string(),
            transport: "Carrier trusted source unavailable".to_string(),
            source_peer: None,
        };
    };

    let channel = if source.channel.trim().is_empty() {
        "stable".to_string()
    } else {
        source.channel.trim().to_string()
    };
    let installed_version = if source.installed_version.trim().is_empty() {
        "unknown".to_string()
    } else {
        source.installed_version.trim().to_string()
    };
    let mode = system_source_mode(&runtime_version, &channel);
    let has_publisher = source
        .publisher_dids
        .iter()
        .any(|publisher| !publisher.trim().is_empty());
    let update_checks_allowed = mode != "development" && has_publisher;
    let update_policy = if !has_publisher {
        "Disabled because the trusted source has no publisher DID.".to_string()
    } else if mode == "development" {
        "Disabled in dev builds; use explicit source/update commands in an operator session."
            .to_string()
    } else {
        format!("Allowed for {mode} mode on the {channel} channel.")
    };
    let source_peer = if source.publisher_node_id.trim().is_empty() {
        None
    } else {
        Some(source.publisher_node_id.trim().to_string())
    };
    let transport = if source_peer.is_some() || !source.connect_ticket.trim().is_empty() {
        "Carrier-first trusted source; web gateways require an explicit operator override."
            .to_string()
    } else {
        "Carrier discovery by publisher DID; web gateways require an explicit operator override."
            .to_string()
    };

    SystemSourceSummary {
        configured: true,
        name: Some(source.name.clone()),
        channel,
        installed_version,
        runtime_version,
        mode: mode.to_string(),
        update_checks_allowed,
        update_policy,
        transport,
        source_peer,
    }
}

fn system_source_mode(runtime_version: &str, channel: &str) -> &'static str {
    let version = runtime_version.to_ascii_lowercase();
    if version.contains("dev") || version.contains("dirty") {
        "development"
    } else if version.contains("rc") || version.contains("review") || channel != "stable" {
        "review"
    } else {
        "release"
    }
}

#[cfg(test)]
mod source_summary_tests {
    use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};

    use super::*;

    #[test]
    fn system_source_mode_keeps_dev_review_and_release_distinct() {
        assert_eq!(system_source_mode("0.5.0-dev", "stable"), "development");
        assert_eq!(system_source_mode("0.5.0", "canary"), "review");
        assert_eq!(system_source_mode("0.5.0-rc1", "stable"), "review");
        assert_eq!(system_source_mode("0.5.0", "stable"), "release");
    }

    #[test]
    fn system_source_summary_blocks_sources_without_publishers() {
        let dir = tempfile::tempdir().unwrap();
        save_trusted_sources(
            dir.path(),
            &TrustedSourcesConfig {
                schema: "elastos.trusted-sources/v1".to_string(),
                default_source: "seed-node-linux".to_string(),
                sources: vec![TrustedSource {
                    name: "seed-node-linux".to_string(),
                    publisher_dids: Vec::new(),
                    channel: "stable".to_string(),
                    discovery_uri: String::new(),
                    connect_ticket: String::new(),
                    gateways: Vec::new(),
                    install_path: String::new(),
                    installed_version: "0.5.0".to_string(),
                    head_cid: String::new(),
                    publisher_node_id: String::new(),
                    ipns_name: String::new(),
                }],
            },
        )
        .unwrap();

        let summary = system_source_summary(
            dir.path(),
            &HomeRuntimeSummary {
                version: Some("0.5.0".to_string()),
                ..HomeRuntimeSummary::default()
            },
        );
        assert_eq!(summary.mode, "release");
        assert!(!summary.update_checks_allowed);
        assert!(summary.update_policy.contains("no publisher DID"));
    }
}

fn system_access_summary(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> SystemAccessSummary {
    let guest_registration_enabled =
        crate::auth::guest_registration_enabled(data_dir).unwrap_or(false);
    let Some(proof_binding_id) = context.proof_binding_id.as_deref() else {
        return SystemAccessSummary {
            role: "local".to_string(),
            guest_registration_enabled,
            ..SystemAccessSummary::default()
        };
    };
    match crate::auth::load_principal_for_proof_binding(data_dir, proof_binding_id) {
        Ok(principal) => SystemAccessSummary {
            role: crate::api::auth_gateway::principal_role_label(principal.role).to_string(),
            localhost_root: Some(principal.localhost_root),
            guest_registration_enabled,
        },
        Err(_) => SystemAccessSummary {
            role: "unknown".to_string(),
            guest_registration_enabled,
            ..SystemAccessSummary::default()
        },
    }
}

pub(super) async fn system_guest_registration_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemGuestRegistrationRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };
    let Some(proof_binding_id) = context.proof_binding_id.as_deref() else {
        return system_error_response(anyhow::anyhow!("admin passkey required"));
    };
    let principal =
        match crate::auth::load_principal_for_proof_binding(&state.data_dir, proof_binding_id) {
            Ok(principal) => principal,
            Err(err) => return system_error_response(err),
        };
    if let Err(err) = crate::auth::ensure_proof_binding_not_revoked(&principal) {
        return system_error_response(err);
    }
    if !crate::auth::is_admin(&principal) {
        return system_error_response(anyhow::anyhow!("admin passkey required"));
    }
    match crate::auth::set_guest_registration_enabled(&state.data_dir, req.enabled, now_ts()) {
        Ok(_) => Json(system_access_summary(&state.data_dir, &context)).into_response(),
        Err(err) => system_error_response(err),
    }
}

struct HomeBackgroundImageEntry {
    path: PathBuf,
    object_uri: String,
    content_type: &'static str,
    version: String,
}

fn home_appearance_summary(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeAppearanceSummary> {
    let (overlay_enabled, overlay_opacity) = home_background_overlay_settings(data_dir, context)?;
    let cache_scope = home_appearance_cache_scope(context);
    Ok(HomeAppearanceSummary {
        background_image_url: home_background_image_entry(data_dir, context)?.map(|entry| {
            format!(
                "/api/apps/home/appearance/background-image?scope={cache_scope}&v={}",
                entry.version
            )
        }),
        background_overlay_enabled: overlay_enabled,
        background_overlay_opacity: overlay_opacity,
    })
}

fn home_appearance_cache_scope(context: &HomeLaunchTokenContext) -> String {
    let digest = Sha256::digest(context.principal_id.as_bytes());
    hex::encode(&digest[..8])
}

fn standard_home_appearance_summary() -> HomeAppearanceSummary {
    HomeAppearanceSummary {
        background_image_url: None,
        background_overlay_enabled: HOME_BACKGROUND_OVERLAY_DEFAULT,
        background_overlay_opacity: HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT,
    }
}

fn home_appearance_root_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/.AppData/ElastOS/Home/Appearance",
        home_browser_localhost_root(context)
    )
}

fn home_appearance_object_uri(context: &HomeLaunchTokenContext, file_name: &str) -> String {
    format!("{}/{}", home_appearance_root_uri(context), file_name)
}

fn home_appearance_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    file_name: &str,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_appearance_object_uri(context, file_name))
        .ok_or_else(|| anyhow::anyhow!("invalid appearance object path"))
}

fn home_background_image_entry(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<Option<HomeBackgroundImageEntry>> {
    for &(file_name, content_type) in HOME_BACKGROUND_IMAGE_FILES {
        let path = home_appearance_path(data_dir, context, file_name)?;
        if !path.is_file() {
            continue;
        }
        let version = path
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|| now_ts().to_string());
        return Ok(Some(HomeBackgroundImageEntry {
            path,
            object_uri: home_appearance_object_uri(context, file_name),
            content_type,
            version,
        }));
    }
    Ok(None)
}

pub(super) fn home_background_overlay_opacity_default() -> f64 {
    HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT
}

fn home_clamp_background_overlay_opacity(opacity: f64) -> f64 {
    if !opacity.is_finite() {
        return HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT;
    }
    opacity.clamp(0.0, HOME_BACKGROUND_OVERLAY_OPACITY_MAX)
}

fn home_background_overlay_settings(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<(bool, f64)> {
    let path = home_appearance_path(data_dir, context, HOME_BACKGROUND_OVERLAY_FILE)?;
    if !path.is_file() {
        return Ok((
            HOME_BACKGROUND_OVERLAY_DEFAULT,
            HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT,
        ));
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_appearance_object_uri(context, HOME_BACKGROUND_OVERLAY_FILE),
        &path,
    )?;
    let payload = serde_json::from_slice::<serde_json::Value>(&bytes)?;
    let enabled = payload
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(HOME_BACKGROUND_OVERLAY_DEFAULT);
    let opacity = payload
        .get("opacity")
        .and_then(|value| value.as_f64())
        .map(home_clamp_background_overlay_opacity)
        .unwrap_or(HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT);
    Ok((enabled, opacity))
}

fn home_save_background_overlay(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    enabled: bool,
    opacity: f64,
) -> anyhow::Result<HomeAppearanceSummary> {
    let path = home_appearance_path(data_dir, context, HOME_BACKGROUND_OVERLAY_FILE)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::json!({
        "enabled": enabled,
        "opacity": home_clamp_background_overlay_opacity(opacity),
    });
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_appearance_object_uri(context, HOME_BACKGROUND_OVERLAY_FILE),
        &path,
        &serde_json::to_vec_pretty(&payload)?,
    )?;
    home_appearance_summary(data_dir, context)
}

fn home_save_background_image(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    file_name: &'static str,
    bytes: Vec<u8>,
) -> anyhow::Result<HomeAppearanceSummary> {
    let path = home_appearance_path(data_dir, context, file_name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_home_background_images(data_dir, context)?;
    crate::auth::write_principal_root_object(
        data_dir,
        &home_browser_principal_id(context),
        &home_browser_localhost_root(context),
        &home_appearance_object_uri(context, file_name),
        &path,
        &bytes,
    )?;
    home_appearance_summary(data_dir, context)
}

fn home_reset_background_image(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeAppearanceSummary> {
    remove_home_background_images(data_dir, context)?;
    home_appearance_summary(data_dir, context)
}

fn remove_home_background_images(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<()> {
    for &(file_name, _content_type) in HOME_BACKGROUND_IMAGE_FILES {
        let path = home_appearance_path(data_dir, context, file_name)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn parse_background_image_upload(
    headers: &HeaderMap,
    body: &Bytes,
) -> anyhow::Result<(&'static str, Vec<u8>)> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("");
    let file_name = match content_type {
        "image/png" => "background-image.png",
        "image/jpeg" => "background-image.jpg",
        "image/webp" => "background-image.webp",
        "image/gif" => "background-image.gif",
        _ => anyhow::bail!("background image must be PNG, JPEG, WebP, or GIF"),
    };
    if body.is_empty() {
        anyhow::bail!("background image is empty");
    }
    if body.len() > HOME_BACKGROUND_IMAGE_MAX_BYTES {
        anyhow::bail!("background image is larger than 5 MB");
    }
    Ok((file_name, body.to_vec()))
}

pub(super) async fn system_handle_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemHandleUpdateRequest>,
) -> Response {
    system_profile_card_update_inner(&state, &headers, req).await
}

pub(super) async fn system_profile_card_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemHandleUpdateRequest>,
) -> Response {
    system_profile_card_update_inner(&state, &headers, req).await
}

async fn system_profile_card_update_inner(
    state: &GatewayState,
    headers: &HeaderMap,
    req: SystemHandleUpdateRequest,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    match update_profile_card_for_context(&state.data_dir, &context, &req.handle) {
        Ok(identity) => Json(identity).into_response(),
        Err(err) => system_error_response(err),
    }
}

fn update_profile_card_for_context(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    handle: &str,
) -> anyhow::Result<HomeIdentitySummary> {
    let Some(proof_binding_id) = context.proof_binding_id.as_deref() else {
        anyhow::bail!("proof-bound passkey session required");
    };
    let principal =
        crate::auth::set_principal_display_name(data_dir, proof_binding_id, handle, now_ts())?;
    home_save_profile_card(
        data_dir,
        context,
        &principal.display_name,
        principal.updated_at,
    )?;
    Ok(load_gateway_identity_summary_for_context(data_dir, context))
}

pub(super) async fn system_background_image_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    let upload = match parse_background_image_upload(&headers, &body) {
        Ok(upload) => upload,
        Err(err) => return system_error_response(err),
    };

    match home_save_background_image(&state.data_dir, &context, upload.0, upload.1) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_background_image_reset(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    match home_reset_background_image(&state.data_dir, &context) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn system_background_overlay_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<SystemBackgroundOverlayRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    match home_save_background_overlay(&state.data_dir, &context, req.enabled, req.opacity) {
        Ok(summary) => Json(summary).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn home_background_image(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };

    let entry = match home_background_image_entry(&state.data_dir, &context) {
        Ok(Some(entry)) => entry,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(err) => return home_error_response(err),
    };
    match crate::auth::read_principal_root_object(
        &state.data_dir,
        &home_browser_principal_id(&context),
        &home_browser_localhost_root(&context),
        &entry.object_uri,
        &entry.path,
    ) {
        Ok(bytes) => {
            let mut response = bytes.into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static(entry.content_type),
            );
            response
        }
        Err(err) => home_error_response(anyhow::anyhow!(err)),
    }
}

#[cfg(test)]
mod home_realtime_tests {
    use super::*;

    #[test]
    fn room_realtime_signature_ignores_session_last_seen_heartbeat() {
        let mut room = HomeRoomSummary {
            active_session_count: 1,
            ..HomeRoomSummary::default()
        };
        room.active_sessions.push(HomeActiveSessionSummary {
            display_name: "Alice".to_string(),
            device_label: "Laptop".to_string(),
            approved_at: 10,
            last_seen_at: 20,
        });
        let before = home_room_realtime_signature(&room);

        room.active_sessions[0].last_seen_at = 30;
        let after = home_room_realtime_signature(&room);

        assert_eq!(
            before, after,
            "presence heartbeat metadata must not emit chat-room.changed events"
        );
    }

    #[test]
    fn people_realtime_signature_ignores_contact_last_seen_heartbeat() {
        let mut people = HomePeopleSummary {
            contact_count: 1,
            contacts: vec![HomePeopleContactSummary {
                contact_id: "contact:alice".to_string(),
                added_at: 10,
                display_name: "Alice".to_string(),
                handle: None,
                relationship: "conversation".to_string(),
                route: "/apps/chat-room/".to_string(),
                can_message: true,
                device_label: Some("MacBook".to_string()),
                profile_card: None,
                last_seen_at: Some(20),
            }],
            ..HomePeopleSummary::default()
        };
        let before = home_people_realtime_signature(&people);

        people.contacts[0].last_seen_at = Some(30);
        let after = home_people_realtime_signature(&people);

        assert_eq!(
            before, after,
            "presence heartbeat metadata must not emit people.changed events"
        );
    }

    #[test]
    fn scoped_realtime_change_does_not_emit_home_summary_event() {
        let snapshot = HomeRealtimeSnapshot {
            principal_id: "person:local:test".to_string(),
            notification_signature: Vec::new(),
            wallet_request_signature: Vec::new(),
            capability_request_count: 0,
            desktop_signature: Vec::new(),
            room_signature: String::new(),
            people_signature: Vec::new(),
            services_signature: Vec::new(),
            browser_sessions: serde_json::json!({
                "schema": "elastos.browser.session-capacity/v1",
                "total_sessions": 0
            }),
        };
        let cursor = home_realtime_cursor(&snapshot);
        let changed = HomeRealtimeSnapshot {
            wallet_request_signature: vec!["request:pending:sign:999".to_string()],
            ..snapshot
        };

        let events = home_realtime_events(&cursor, &changed);

        assert!(
            events
                .iter()
                .any(|event| event.kind == "wallet.requests.changed" && event.scope == "wallet"),
            "wallet changes should still emit wallet scoped events"
        );
        assert!(
            !events
                .iter()
                .any(|event| event.kind == "home.summary.changed"),
            "scoped provider changes must not force full Home summary refreshes"
        );
    }

    #[test]
    fn people_realtime_change_emits_people_scoped_event_only() {
        let snapshot = HomeRealtimeSnapshot {
            principal_id: "person:local:test".to_string(),
            notification_signature: Vec::new(),
            wallet_request_signature: Vec::new(),
            capability_request_count: 0,
            desktop_signature: Vec::new(),
            room_signature: String::new(),
            people_signature: vec!["contact:alice:Alice".to_string()],
            services_signature: Vec::new(),
            browser_sessions: serde_json::json!({
                "schema": "elastos.browser.session-capacity/v1",
                "total_sessions": 0
            }),
        };
        let cursor = home_realtime_cursor(&snapshot);
        let changed = HomeRealtimeSnapshot {
            people_signature: vec!["contact:alice:Alice", "contact:bob:Bob"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            ..snapshot
        };

        let events = home_realtime_events(&cursor, &changed);

        assert!(
            events
                .iter()
                .any(|event| event.kind == "people.changed" && event.scope == "people"),
            "people changes should emit a People-scoped event"
        );
        assert!(
            !events
                .iter()
                .any(|event| event.kind == "home.summary.changed"),
            "People changes must not force a generic Home summary event"
        );
    }
}
