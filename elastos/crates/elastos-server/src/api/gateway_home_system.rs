use std::collections::{BTreeMap, BTreeSet};

use elastos_common::CapsuleManifest;

use super::*;

const HOME_EVENTS_SCHEMA: &str = "elastos.home.events/v1";
const HOME_EVENTS_DEFAULT_WAIT_MS: u64 = 25_000;
const HOME_EVENTS_MAX_WAIT_MS: u64 = 30_000;
const HOME_EVENTS_POLL_MS: u64 = 1_000;
const HOME_EVENTS_RETRY_MS: u64 = 250;
const HOME_EVENTS_STREAM_KEEPALIVE_SECS: u64 = 15;
const HOME_DESKTOP_OBJECTS_SCHEMA: &str = "elastos.home.desktop-objects/v1";
const HOME_SYSTEM_DESKTOP_OBJECT_SCHEMA: &str = "elastos.home.system-desktop-object/v1";

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
    browser_sessions: serde_json::Value,
}

pub(super) async fn home_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = require_home_token_context(&state.data_dir, &headers).ok();

    let (identity, authority, browser_state, appearance, runtime, home_state) =
        if let Some(context) = context.as_ref() {
            let identity = load_gateway_identity_summary_for_context(&state.data_dir, context);
            let data_dir = state.data_dir.clone();
            let (runtime, home_state) =
                tokio::join!(home_runtime_summary(&state.data_dir), async move {
                    tokio::task::spawn_blocking(move || home_state(&data_dir))
                        .await
                        .unwrap_or_default()
                });
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
    let desktop_objects = if let Some(context) = context.as_ref() {
        home_desktop_objects_summary(&state, context).await
    } else {
        standard_home_desktop_objects_summary()
    };
    if let Some(context) = context.as_ref() {
        let wallet_approvals =
            system_wallet_approvals_summary(&state, &context.principal_id, false).await;
        append_wallet_approval_notifications(
            &mut notifications,
            wallet_approvals.approval_requests,
        );
        if let Ok(capability_requests) = runtime_capability_pending_requests(&state.data_dir).await
        {
            append_runtime_capability_notifications(&mut notifications, capability_requests);
        }
    }

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
        appearance,
        runtime,
        site: home_state.site,
        room: home_state.room,
        notifications,
        desktop_objects,
        targets: home_targets(&state.data_dir),
    })
    .into_response()
}

pub(super) async fn home_events(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(query): Query<HomeEventsQuery>,
) -> Response {
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    let previous_cursor = query.cursor.unwrap_or_default();
    let wait_ms = query
        .wait_ms
        .unwrap_or(HOME_EVENTS_DEFAULT_WAIT_MS)
        .min(HOME_EVENTS_MAX_WAIT_MS);
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let snapshot = home_realtime_snapshot(&state, &context).await;
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
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    let stream_state = HomeEventsStreamState {
        state,
        context,
        cursor: String::new(),
    };
    let stream = futures_lite::stream::unfold(stream_state, |mut stream_state| async move {
        loop {
            let snapshot = home_realtime_snapshot(&stream_state.state, &stream_state.context).await;
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
) -> HomeRealtimeSnapshot {
    let data_dir = state.data_dir.clone();
    let home_state = tokio::task::spawn_blocking(move || home_state(&data_dir))
        .await
        .unwrap_or_default();
    let room_signature = home_room_realtime_signature(&home_state.room);
    let mut notifications = home_state.notifications;
    let wallet_approvals =
        system_wallet_approvals_summary(state, &context.principal_id, false).await;
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
    )
    .await;
    let desktop_signature = home_desktop_events_signature(state, context).await;
    HomeRealtimeSnapshot {
        principal_id: context.principal_id.clone(),
        notification_signature,
        wallet_request_signature,
        capability_request_count,
        desktop_signature,
        room_signature,
        browser_sessions,
    }
}

fn home_realtime_cursor(snapshot: &HomeRealtimeSnapshot) -> String {
    let parts = home_realtime_cursor_parts(snapshot);
    format!(
        "v1:home={};inbox={};wallet={};browser={};desktop={};chat-room={}",
        parts.home, parts.inbox, parts.wallet, parts.browser, parts.desktop, parts.chat_room
    )
}

struct HomeRealtimeCursorParts {
    home: String,
    inbox: String,
    wallet: String,
    browser: String,
    desktop: String,
    chat_room: String,
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
            // poll/event feedback loop in Home-launched Chat Room windows.
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
    let context = match require_home_token_context(&state.data_dir, &headers) {
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
    let context = match require_home_token_context(&state.data_dir, &headers) {
        Ok(context) => context,
        Err(err) => return home_error_response(err),
    };
    match home_save_browser_state(&state.data_dir, &context, input) {
        Ok(state) => Json(state).into_response(),
        Err(err) => home_error_response(err),
    }
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
    }
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
        Err(err) if is_unencrypted_home_browser_state(&err) => {
            return Ok(default_home_browser_state(context));
        }
        Err(err) if is_missing_home_browser_state_file(&err) => {
            return Ok(default_home_browser_state(context));
        }
        Err(err) => return Err(err),
    };
    // browser-state.json carries NO authority — only cosmetic UI state (recent
    // targets + window layout). A corrupt/mismatched file (e.g. the desktop app
    // rewriting it non-atomically, leaving valid JSON + trailing bytes) must never
    // fail-close the Home summary and block sign-in. So a parse/schema/principal/root
    // failure resets to default with a warning, exactly like the missing/unencrypted
    // fallbacks above. Our own writer uses atomic_write, so the runtime never creates
    // this corruption.
    let mut state: HomeBrowserStateSummary = match serde_json::from_slice(&bytes) {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(
                "resetting corrupt Home browser-state (cosmetic, no authority): {err}"
            );
            return Ok(default_home_browser_state(context));
        }
    };
    if state.schema != HOME_BROWSER_STATE_SCHEMA
        || state.principal_id != principal_id
        || state.localhost_root != localhost_root
    {
        tracing::warn!(
            "resetting mismatched Home browser-state (schema/principal/root mismatch) — cosmetic, no authority"
        );
        return Ok(default_home_browser_state(context));
    }
    state.recent_targets = sanitize_recent_targets(state.recent_targets);
    sanitize_home_browser_state_targets(data_dir, &mut state);
    Ok(state)
}

fn is_unencrypted_home_browser_state(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .to_string()
            .contains(crate::auth::PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED)
    })
}

fn is_missing_home_browser_state_file(err: &anyhow::Error) -> bool {
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
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    let (runtime, storage, wallet_accounts, wallet_approvals, runtime_log) = tokio::join!(
        home_runtime_summary(&state.data_dir),
        system_storage_summary(
            state.provider_registry.as_ref().cloned(),
            &context.principal_id
        ),
        system_wallet_accounts_summary(&state, &context.principal_id),
        system_wallet_approvals_summary(&state, &context.principal_id, false),
        system_runtime_log(&state.data_dir)
    );
    let webspace = system_webspace_summary(&state.data_dir, &runtime);
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
        runtime,
        storage,
        webspace,
        wallet_accounts,
        wallet_approvals,
        runtime_log,
    })
    .into_response()
}

fn system_webspace_summary(
    data_dir: &std::path::Path,
    runtime: &HomeRuntimeSummary,
) -> SystemWebspaceSummary {
    let running = runtime
        .running_capsules
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut entries = crate::api::capsule_inventory::list_capsule_manifests(data_dir)
        .into_iter()
        .map(|manifest| {
            let role = manifest.role.as_str().to_string();
            let capsule_type = capsule_type_label(&manifest.capsule_type).to_string();
            let provides = manifest.provides.clone();
            let uri = provides
                .clone()
                .unwrap_or_else(|| format!("elastos://capsules/{}", manifest.name));
            let operations = manifest
                .authority
                .as_ref()
                .map(|authority| {
                    let mut operations = authority
                        .capabilities
                        .iter()
                        .flat_map(|capability| capability.operations.iter().cloned())
                        .collect::<BTreeSet<_>>();
                    if operations.is_empty() {
                        operations.insert("capability-gated".to_string());
                    }
                    operations.into_iter().collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let route = if manifest.role.is_shell_launchable() {
                Some(format!("/apps/{}/", manifest.name))
            } else {
                None
            };
            let is_running = running.contains(&manifest.name);
            let status = system_webspace_status(&manifest, is_running).to_string();
            let backend = system_webspace_backend(&manifest).to_string();
            let authority_boundary = system_webspace_boundary(&manifest);
            SystemWebspaceEntry {
                id: manifest.name.clone(),
                role,
                capsule_type,
                uri,
                provides,
                capabilities: manifest.capabilities,
                operations,
                route,
                running: is_running,
                status,
                backend,
                authority_boundary,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        system_webspace_sort_rank(&left.role, &left.id)
            .cmp(&system_webspace_sort_rank(&right.role, &right.id))
    });
    SystemWebspaceSummary { entries }
}

fn system_webspace_sort_rank(role: &str, id: &str) -> (u8, String) {
    let rank = match role {
        "shell" => 0,
        "app" => 1,
        "viewer" => 2,
        "provider" => 3,
        "content" => 4,
        _ => 5,
    };
    (rank, id.to_string())
}

fn capsule_type_label(capsule_type: &CapsuleType) -> &'static str {
    match capsule_type {
        CapsuleType::Wasm => "wasm",
        CapsuleType::MicroVM => "microvm",
        CapsuleType::Oci => "oci",
        CapsuleType::Media => "media",
        CapsuleType::Data => "data",
    }
}

fn system_webspace_status(manifest: &CapsuleManifest, running: bool) -> &'static str {
    if running {
        return "running";
    }
    if manifest.role == CapsuleRole::Provider {
        return "available";
    }
    "installed"
}

fn system_webspace_backend(manifest: &CapsuleManifest) -> &'static str {
    if manifest.role != CapsuleRole::Provider {
        return "capsule";
    }
    let provides = manifest.provides.as_deref().unwrap_or_default();
    if provides.starts_with("elastos://browser-engine/") {
        "Browser Engine provider"
    } else if provides.starts_with("elastos://net/") || provides.starts_with("elastos://exit/") {
        "Net/Exit provider"
    } else if provides.starts_with("elastos://wallet/") {
        "Wallet authority provider"
    } else if provides.starts_with("elastos://chain/") {
        "Typed chain provider"
    } else if provides.starts_with("elastos://ipfs/")
        || provides.starts_with("elastos://availability/")
        || provides.starts_with("localhost://WebSpaces/")
    {
        "Content/Webspace provider"
    } else {
        "provider"
    }
}

fn system_webspace_boundary(manifest: &CapsuleManifest) -> String {
    if let Some(authority) = manifest.authority.as_ref() {
        return authority.reason.clone();
    }
    if manifest.role == CapsuleRole::Provider {
        return "Provider capsule: concrete backend access stays behind the declared URI and capability schema.".to_string();
    }
    "User-facing capsule: Runtime grants scoped launch/capability access; raw provider authority stays outside the app.".to_string()
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

pub(super) async fn system_storage_summary(
    provider_registry: Option<Arc<ProviderRegistry>>,
    principal_id: &str,
) -> SystemStorageSummary {
    let Some(registry) = provider_registry else {
        return SystemStorageSummary {
            available: false,
            note: Some("Document provider unavailable.".to_string()),
            ..SystemStorageSummary::default()
        };
    };
    match DocumentsClient::for_principal(registry, principal_id)
        .summary()
        .await
    {
        Ok(documents) => {
            let published_count = documents
                .iter()
                .filter(|item| {
                    !item
                        .latest_published_cid
                        .as_deref()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                })
                .count();
            let documents_count = documents.len();
            SystemStorageSummary {
                available: true,
                documents_count,
                drafts_count: documents_count.saturating_sub(published_count),
                published_count,
                objects_root: Some("localhost://ElastOS/Documents/".to_string()),
                note: Some("Documents stay local until published.".to_string()),
            }
        }
        Err(err) => SystemStorageSummary {
            available: false,
            note: Some(err.to_string()),
            ..SystemStorageSummary::default()
        },
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
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return system_error_response(err),
        };

    let Some(proof_binding_id) = context.proof_binding_id.as_deref() else {
        return system_error_response(anyhow::anyhow!("proof-bound passkey session required"));
    };

    match crate::auth::set_principal_display_name(
        &state.data_dir,
        proof_binding_id,
        &req.handle,
        now_ts(),
    ) {
        Ok(_) => Json(load_gateway_identity_summary_for_context(
            &state.data_dir,
            &context,
        ))
        .into_response(),
        Err(err) => system_error_response(err),
    }
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
    fn scoped_realtime_change_does_not_emit_home_summary_event() {
        let snapshot = HomeRealtimeSnapshot {
            principal_id: "person:local:test".to_string(),
            notification_signature: Vec::new(),
            wallet_request_signature: Vec::new(),
            capability_request_count: 0,
            desktop_signature: Vec::new(),
            room_signature: String::new(),
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
}
