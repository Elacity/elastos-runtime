//! Browser admission uses existing Runtime sessions, pending approvals and
//! capability signatures. Home tokens authorize decisions and remain private.
use super::*;
use elastos_common::browser_protocol::{BrowserOperatorRequest, BrowserRefInput};
use elastos_runtime::capability::pending::PendingRequestStore;
use elastos_runtime::capability::{
    Action, CapabilityManager, CapabilityStore, CapabilityToken, GrantDuration, ResourceId,
    TokenConstraints,
};
use elastos_runtime::primitives::{
    audit::AuditLog, metrics::MetricsManager, time::SecureTimestamp,
};
use elastos_runtime::session::{Session, SessionRegistry};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use tokio::time::Instant;

static SERVICES: OnceLock<StdMutex<BTreeMap<PathBuf, Weak<BrowserOperatorService>>>> =
    OnceLock::new();

pub(crate) struct BrowserOperatorService {
    sessions: Arc<SessionRegistry>,
    pending: PendingRequestStore,
    capabilities: CapabilityManager,
    records: tokio::sync::Mutex<BTreeMap<String, Admission>>,
    snapshots: tokio::sync::Mutex<BTreeMap<String, OperatorSnapshot>>,
}

#[derive(Clone)]
struct OperatorSnapshot {
    owner: BrowserInspectionOwner,
    id: String,
    generation: String,
    refs: BTreeSet<String>,
    expires: Instant,
}

struct Admission {
    page_id: String,
    session: Session,
    request: BrowserOperatorRequest,
    created: Instant,
    owner: Option<BrowserInspectionOwner>,
    owner_token: String,
    snapshot: Option<OperatorSnapshot>,
    token: Option<CapabilityToken>,
    expires: Instant,
    phase: &'static str,
    busy: bool,
    release_pending: bool,
    receipts: BTreeMap<String, serde_json::Value>,
    bindings: BTreeMap<String, String>,
}

pub(crate) fn register_browser_operator_sessions(
    data_dir: &FsPath,
    sessions: Arc<SessionRegistry>,
) -> Arc<BrowserOperatorService> {
    let audit = Arc::new(AuditLog::new());
    let service = Arc::new(BrowserOperatorService {
        sessions,
        pending: PendingRequestStore::with_timeout(audit.clone(), 60),
        capabilities: CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            audit,
            Arc::new(MetricsManager::new()),
        ),
        records: Default::default(),
        snapshots: Default::default(),
    });
    let mut services = SERVICES.get_or_init(Default::default).lock().unwrap();
    services.retain(|_, value| value.strong_count() > 0);
    services.insert(data_dir.to_path_buf(), Arc::downgrade(&service));
    service
}

fn service(state: &GatewayState) -> Result<Arc<BrowserOperatorService>, Response> {
    SERVICES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&state.data_dir)
        .and_then(Weak::upgrade)
        .ok_or_else(|| {
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "operator_sessions_unavailable",
            )
        })
}
pub(super) async fn remember_inspection(
    state: &GatewayState,
    owner: &BrowserInspectionOwner,
    value: &serde_json::Value,
    started: Instant,
) {
    let Ok(service) = service(state) else {
        return;
    };
    if value["schema"] != "elastos.browser.inspect-result/v1" {
        return;
    }
    let mut snapshots = service.snapshots.lock().await;
    snapshots.retain(|_, s| s.expires > Instant::now());
    if snapshots.len() >= 128 && !snapshots.contains_key(owner.page_id()) {
        return;
    }
    let id = value["snapshot_id"].as_str().unwrap().to_string();
    let generation = value["document_generation"].as_str().unwrap().to_string();
    if snapshots
        .get(owner.page_id())
        .is_none_or(|s| s.owner != *owner || s.id != id || s.generation != generation)
    {
        snapshots.insert(
            owner.page_id().to_string(),
            OperatorSnapshot {
                owner: owner.clone(),
                id,
                generation,
                refs: Default::default(),
                expires: started + std::time::Duration::from_secs(30),
            },
        );
    }
    let snapshot = snapshots.get_mut(owner.page_id()).unwrap();
    for node in value["nodes"].as_array().unwrap() {
        snapshot
            .refs
            .insert(node["ref"].as_str().unwrap().to_string());
    }
}

fn service_current(state: &GatewayState, expected: &Arc<BrowserOperatorService>) -> bool {
    service(state).is_ok_and(|current| Arc::ptr_eq(&current, expected))
}

fn failure(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"schema":"elastos.browser.operator-error/v1","code":code})),
    )
        .into_response()
}
async fn operator_session(
    service: &BrowserOperatorService,
    headers: &HeaderMap,
) -> Result<Session, Response> {
    if headers.contains_key("x-elastos-home-token") || headers.contains_key(COOKIE) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "conflicting_operator_authority",
        ));
    }
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() <= 128)
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "operator_session_required"))?;
    service
        .sessions
        .validate_token(token)
        .await
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED, "operator_session_expired"))
}
fn resource(page: &str, id: &str) -> ResourceId {
    ResourceId::new(format!("elastos://browser/pages/{page}/operator/{id}"))
}
async fn owner_live(state: &GatewayState, owner: &BrowserInspectionOwner, token: &str) -> bool {
    require_carried_home_launch_token(&state.data_dir, token, &[BROWSER_CAPSULE_ID]).is_ok_and(
        |grant| {
            grant.context.principal_id == owner.principal_id()
                && grant.launch_id == owner.launch_id()
        },
    ) && browser_inspection_owner_current(&state.data_dir, owner).await
}
async fn engine_input(
    state: &GatewayState,
    owner: &BrowserInspectionOwner,
    event: serde_json::Value,
) -> Result<serde_json::Value, Response> {
    let registration = match state.provider_registry.as_ref() {
        Some(registry) => {
            registry
                .registration_for_uri("elastos://browser-engine/page/input")
                .await
        }
        None => None,
    };
    if (registration.is_none_or(|r| r.provider != owner.engine_route_provider)
        && !super::gateway_browser_remote::route_matches(
            state,
            owner.principal_id(),
            owner.page_id(),
            &owner.engine_route_provider,
        ))
        || !browser_inspection_owner_current(&state.data_dir, owner).await
    {
        return Err(failure(StatusCode::CONFLICT, "operator_owner_changed"));
    }
    if event["type"] == "operator_ref"
        || (event["type"] == "operator_lease" && event["command"] == "acquire")
    {
        let service = service(state)?;
        let records = service.records.lock().await;
        let record = records
            .get(event["admission_id"].as_str().unwrap_or_default())
            .filter(|r| {
                r.owner.as_ref() == Some(owner)
                    && r.expires > Instant::now()
                    && if event["type"] == "operator_ref" {
                        r.phase == "active" && r.busy
                    } else {
                        r.phase == "approving"
                    }
            })
            .ok_or_else(|| failure(StatusCode::FORBIDDEN, "operator_admission_inactive"))?;
        if service
            .sessions
            .validate_token(&record.session.token)
            .await
            .is_none()
            || !owner_live(state, owner, &record.owner_token).await
            || !service_current(state, &service)
        {
            return Err(failure(
                StatusCode::FORBIDDEN,
                "operator_admission_inactive",
            ));
        }
    }
    let expected = event.clone();
    let call = browser_provider_resource_call("browser-engine", "input", "elastos://browser-engine/page/input".into(),
        serde_json::json!({"page_id":owner.page_id(),"principal_id":owner.principal_id(),"event":event}))
        .map_err(|_| failure(StatusCode::BAD_GATEWAY, "operator_dispatch_failed"))?;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        browser_provider_resource_response(state, call),
    )
    .await
    .map_err(|_| failure(StatusCode::GATEWAY_TIMEOUT, "operator_outcome_uncertain"))?
    .map_err(|_| failure(StatusCode::BAD_GATEWAY, "operator_outcome_uncertain"))?;
    let value = provider_response_data(&response)
        .ok_or_else(|| failure(StatusCode::BAD_GATEWAY, "operator_outcome_uncertain"))?;
    if value["page_id"] != owner.page_id() || value["accepted"] != true {
        return Err(failure(StatusCode::CONFLICT, "operator_input_rejected"));
    }
    let exact = match expected["type"].as_str() {
        Some("operator_lease") => {
            value["schema"] == "elastos.browser.input-result/v1"
                && value["admission_id"] == expected["admission_id"]
                && value["writer_acquired"] == (expected["command"] == "acquire")
        }
        Some("operator_ref") => {
            value["schema"] == "elastos.browser.ref-input-result/v1"
                && value["admission_id"] == expected["admission_id"]
                && value["request_id"] == expected["request_id"]
                && value["document_generation"] == expected["document_generation"]
        }
        _ => false,
    };
    if !exact {
        return Err(failure(
            StatusCode::BAD_GATEWAY,
            "operator_receipt_mismatch",
        ));
    }
    if expected["type"] == "operator_lease" && expected["command"] == "release" {
        if let Ok(service) = service(state) {
            if let Some(record) = service
                .records
                .lock()
                .await
                .get_mut(expected["admission_id"].as_str().unwrap_or_default())
            {
                if record.owner.as_ref() == Some(owner) {
                    record.release_pending = false;
                }
            }
        }
    }
    Ok(value)
}

pub(in crate::api::gateway) async fn request_admission(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
    Json(request): Json<BrowserOperatorRequest>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session = match operator_session(&service, &headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !is_safe_runtime_id(&page_id) || !request.valid() {
        return failure(StatusCode::BAD_REQUEST, "invalid_operator_request");
    }
    let mut records = service.records.lock().await;
    let now = Instant::now();
    records.retain(|_, r| {
        now.duration_since(r.created).as_secs() < 120
            || r.busy
            || r.release_pending
            || r.phase == "approving"
    });
    if records.len() >= 128
        || records
            .values()
            .filter(|r| r.session.id == session.id)
            .count()
            >= 4
    {
        return failure(StatusCode::TOO_MANY_REQUESTS, "operator_requests_busy");
    }
    service.pending.cleanup_old(60).await;
    let pending = service
        .pending
        .create_request_with_reason(
            session.id.clone(),
            ResourceId::new(format!("elastos://browser/pages/{page_id}/input")),
            Action::Write,
            request.reason.clone(),
        )
        .await;
    if !pending.is_pending() {
        return failure(StatusCode::TOO_MANY_REQUESTS, "operator_requests_busy");
    }
    let id = pending.id.to_string();
    records.insert(
        id.clone(),
        Admission {
            page_id,
            session,
            request,
            created: now,
            owner: None,
            owner_token: String::new(),
            snapshot: None,
            token: None,
            expires: now,
            phase: "pending",
            busy: false,
            release_pending: false,
            receipts: Default::default(),
            bindings: Default::default(),
        },
    );
    Json(serde_json::json!({"schema":"elastos.browser.operator-admission/v1","request_id":id,"status":"pending"})).into_response()
}

pub(in crate::api::gateway) async fn admission_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path((page_id, id)): Path<(String, String)>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session = match operator_session(&service, &headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let records = service.records.lock().await;
    let Some(record) = records
        .get(&id)
        .filter(|r| r.page_id == page_id && r.session.id == session.id)
    else {
        return failure(StatusCode::NOT_FOUND, "operator_request_unavailable");
    };
    let live = record.phase == "active"
        && Instant::now() < record.expires
        && match record.owner.as_ref() {
            Some(owner) => owner_live(&state, owner, &record.owner_token).await,
            None => false,
        };
    Json(serde_json::json!({"schema":"elastos.browser.operator-admission/v1","request_id":id,
        "status":if live {"active"} else if record.phase == "active" {"expired"} else {record.phase},
        "capability":if live {record.token.as_ref().and_then(|t|t.to_base64().ok())} else {None},
        "writer_acquired":live,"receipts":record.receipts.values().collect::<Vec<_>>() })).into_response()
}

pub(in crate::api::gateway) async fn pending_admissions(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(page_id): Path<String>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(a) => a,
            Err(e) => return gateway_provider_error_response("browser", e),
        };
    if capture_browser_inspection_owner(
        &state.data_dir,
        &page_id,
        authority.verified_context().principal_id(),
        authority.verified_context().launch_id(),
    )
    .await
    .is_none()
    {
        return failure(StatusCode::NOT_FOUND, "operator_owner_unavailable");
    }
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let records = service.records.lock().await;
    let requests: Vec<_> = records.iter().filter(|(_,r)|r.page_id == page_id && r.phase == "pending" && r.created.elapsed().as_secs() < 60)
        .map(|(id,r)|serde_json::json!({"request_id":id,"operator_session_id":r.session.id,"request":r.request})).collect();
    Json(serde_json::json!({"schema":"elastos.browser.operator-pending/v1","requests":requests}))
        .into_response()
}

pub(in crate::api::gateway) async fn approve_admission(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path((page_id, id)): Path<(String, String)>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(a) => a,
            Err(e) => return gateway_provider_error_response("browser", e),
        };
    let Some(owner) = capture_browser_inspection_owner(
        &state.data_dir,
        &page_id,
        authority.verified_context().principal_id(),
        authority.verified_context().launch_id(),
    )
    .await
    else {
        return failure(StatusCode::NOT_FOUND, "operator_owner_unavailable");
    };
    let owner_token = home_launch_token_header(&headers).unwrap_or_default();
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let snapshot = service
        .snapshots
        .lock()
        .await
        .get(&page_id)
        .filter(|s| s.owner == owner && s.expires > Instant::now())
        .cloned();
    let Some(snapshot) = snapshot else {
        return failure(StatusCode::CONFLICT, "operator_inspection_required");
    };
    let (session, request, expires) = {
        let mut records = service.records.lock().await;
        if records.iter().any(|(key, r)| {
            key != &id
                && r.page_id == page_id
                && (matches!(r.phase, "approving" | "active") || r.release_pending)
                && (r.release_pending || r.phase == "approving" || r.expires > Instant::now())
        }) {
            return failure(StatusCode::CONFLICT, "operator_writer_busy");
        }
        let Some(record) = records.get_mut(&id).filter(|r| {
            r.page_id == page_id && r.phase == "pending" && r.created.elapsed().as_secs() < 60
        }) else {
            return failure(StatusCode::NOT_FOUND, "operator_request_unavailable");
        };
        if service
            .sessions
            .validate_token(&record.session.token)
            .await
            .is_none()
        {
            return failure(StatusCode::UNAUTHORIZED, "operator_session_expired");
        }
        if record.request.document_generation != snapshot.generation {
            return failure(StatusCode::CONFLICT, "stale_inspection");
        }
        record.snapshot = Some(snapshot.clone());
        record.phase = "approving";
        record.release_pending = true;
        record.owner = Some(owner.clone());
        record.owner_token = owner_token.clone();
        record.expires =
            Instant::now() + std::time::Duration::from_millis(record.request.duration_ms.into());
        (
            record.session.clone(),
            record.request.clone(),
            record.expires,
        )
    };
    let result = engine_input(&state,&owner,serde_json::json!({"type":"operator_lease","command":"acquire","admission_id":id,
        "document_generation":request.document_generation,"duration_ms":request.duration_ms,"actions":request.actions})).await;
    let live = service_current(&state, &service)
        && owner_live(&state, &owner, &owner_token).await
        && service
            .sessions
            .validate_token(&session.token)
            .await
            .is_some();
    let mut records = service.records.lock().await;
    let record = records.get_mut(&id).unwrap();
    if result.is_err() || !live || record.phase != "approving" || Instant::now() >= expires {
        record.phase = "revoked";
        drop(records);
        let _ = engine_input(
            &state,
            &owner,
            serde_json::json!({"type":"operator_lease","command":"release","admission_id":id}),
        )
        .await;
        return result
            .err()
            .unwrap_or_else(|| failure(StatusCode::CONFLICT, "operator_owner_changed"));
    }
    let token = service.capabilities.grant(
        session.id.as_str(),
        resource(&page_id, &id),
        Action::Write,
        TokenConstraints::new(
            service.capabilities.current_epoch(),
            false,
            None,
            Some(request.max_actions.into()),
        ),
        Some(SecureTimestamp::after_secs(
            u64::from(request.duration_ms).div_ceil(1000),
        )),
    );
    if service
        .pending
        .grant_request(&id, token.clone(), GrantDuration::Session)
        .await
        .is_err()
    {
        record.phase = "revoked";
        drop(records);
        let _ = engine_input(
            &state,
            &owner,
            serde_json::json!({"type":"operator_lease","command":"release","admission_id":id}),
        )
        .await;
        return failure(StatusCode::CONFLICT, "operator_request_expired");
    }
    record.token = Some(token);
    record.phase = "active";
    Json(serde_json::json!({"schema":"elastos.browser.operator-admission/v1","request_id":id,"status":"active","writer_acquired":true})).into_response()
}

pub(in crate::api::gateway) async fn revoke_admission(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path((page_id, id)): Path<(String, String)>,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[BROWSER_CAPSULE_ID]) {
            Ok(a) => a,
            Err(e) => return gateway_provider_error_response("browser", e),
        };
    let Some(owner) = capture_browser_inspection_owner(
        &state.data_dir,
        &page_id,
        authority.verified_context().principal_id(),
        authority.verified_context().launch_id(),
    )
    .await
    else {
        return failure(StatusCode::NOT_FOUND, "operator_owner_unavailable");
    };
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let release_pending = {
        let mut records = service.records.lock().await;
        let Some(record) = records.get_mut(&id).filter(|r| r.page_id == page_id) else {
            return failure(StatusCode::NOT_FOUND, "operator_request_unavailable");
        };
        record.phase = "revoked";
        if let Some(token) = &record.token {
            service
                .capabilities
                .revoke(*token.id(), "Browser owner revoked admission")
                .await;
        }
        service.pending.revoke_request(&id).await;
        record.release_pending
    };
    if !release_pending {
        return Json(serde_json::json!({"schema":"elastos.browser.operator-admission/v1","request_id":id,"status":"revoked","writer_acquired":false})).into_response();
    }
    match engine_input(&state,&owner,serde_json::json!({"type":"operator_lease","command":"release","admission_id":id})).await {
        Ok(_)=>Json(serde_json::json!({"schema":"elastos.browser.operator-admission/v1","request_id":id,"status":"revoked","writer_acquired":false})).into_response(),Err(e)=>e,
    }
}

pub(in crate::api::gateway) async fn return_page_to_owner(
    state: &GatewayState,
    page_id: &str,
    principal: &str,
    launch: &str,
) -> Result<(), Response> {
    let Ok(service) = service(state) else {
        return Ok(());
    };
    if !service.records.lock().await.values().any(|r| {
        r.page_id == page_id && (matches!(r.phase, "active" | "approving") || r.release_pending)
    }) {
        return Ok(());
    }
    let Some(owner) =
        capture_browser_inspection_owner(&state.data_dir, page_id, principal, launch).await
    else {
        return Err(failure(StatusCode::CONFLICT, "operator_owner_changed"));
    };
    let ids = {
        let mut records = service.records.lock().await;
        records
            .iter_mut()
            .filter(|(_, r)| {
                r.page_id == page_id
                    && (matches!(r.phase, "active" | "approving") || r.release_pending)
            })
            .map(|(id, r)| {
                r.phase = "revoked";
                id.clone()
            })
            .collect::<Vec<_>>()
    };
    for id in ids {
        if let Some(token) = service
            .records
            .lock()
            .await
            .get(&id)
            .and_then(|r| r.token.clone())
        {
            service
                .capabilities
                .revoke(*token.id(), "Browser owner reclaimed input")
                .await;
            service.pending.revoke_request(&id).await;
        }

        engine_input(
            state,
            &owner,
            serde_json::json!({"type":"operator_lease","command":"release","admission_id":id}),
        )
        .await?;
    }
    Ok(())
}

pub(in crate::api::gateway) async fn operator_input(
    state: GatewayState,
    headers: HeaderMap,
    page_id: String,
    event: serde_json::Value,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let session = match operator_session(&service, &headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let input: BrowserRefInput = match serde_json::from_value(event) {
        Ok(v) => v,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_operator_input"),
    };
    if !input.valid() {
        return failure(StatusCode::BAD_REQUEST, "invalid_operator_input");
    }
    let capability = match headers
        .get("x-elastos-capability")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 8192)
        .and_then(|v| CapabilityToken::from_base64(v).ok())
    {
        Some(t) => t,
        None => return failure(StatusCode::UNAUTHORIZED, "operator_capability_required"),
    };
    let (owner, owner_token) = {
        let mut records = service.records.lock().await;
        let Some(record) = records
            .get_mut(&input.admission_id)
            .filter(|r| r.page_id == page_id && r.session.id == session.id)
        else {
            return failure(StatusCode::NOT_FOUND, "operator_admission_unavailable");
        };
        if record.phase != "active"
            || record.expires <= Instant::now()
            || record.request.document_generation != input.document_generation
            || !record.request.actions.contains(&input.action)
            || record
                .token
                .as_ref()
                .is_none_or(|t| t.to_base64().ok() != capability.to_base64().ok())
        {
            return failure(StatusCode::FORBIDDEN, "operator_admission_inactive");
        }
        if record.snapshot.as_ref().is_none_or(|snapshot| {
            snapshot.expires <= Instant::now()
                || snapshot.generation != input.document_generation
                || !snapshot.refs.contains(&input.reference)
        }) {
            return failure(StatusCode::CONFLICT, "stale_inspection");
        }
        let owner = record.owner.as_ref().unwrap().clone();
        if !owner_live(&state, &owner, &record.owner_token).await {
            return failure(StatusCode::CONFLICT, "operator_owner_changed");
        }
        // A repeated request returns its bounded receipt, including uncertainty,
        // without consuming another use or dispatching again.
        if let Some(receipt) = record.receipts.get(&input.request_id) {
            if record.bindings.get(&input.request_id)
                != Some(&serde_json::to_string(&input).unwrap())
            {
                return failure(StatusCode::CONFLICT, "operator_request_conflict");
            }
            return Json(receipt.clone()).into_response();
        }
        if record.busy {
            return failure(StatusCode::CONFLICT, "operator_writer_busy");
        }
        if service
            .capabilities
            .validate(
                &capability,
                session.id.as_str(),
                Action::Write,
                &resource(&page_id, &input.admission_id),
                None,
            )
            .await
            .is_err()
        {
            return failure(StatusCode::FORBIDDEN, "operator_capability_rejected");
        }
        record.busy = true;
        record.bindings.insert(
            input.request_id.clone(),
            serde_json::to_string(&input).unwrap(),
        );
        record.receipts.insert(input.request_id.clone(),serde_json::json!({"schema":"elastos.browser.ref-input-result/v1","page_id":page_id,"request_id":input.request_id,"accepted":false,"outcome":"pending"}));
        (owner, record.owner_token.clone())
    };
    let mut event = serde_json::to_value(&input).unwrap();
    event["type"] = serde_json::json!("operator_ref");
    let outcome = engine_input(&state, &owner, event).await;
    let live = service_current(&state, &service)
        && owner_live(&state, &owner, &owner_token).await
        && service
            .sessions
            .validate_token(&session.token)
            .await
            .is_some();
    let mut records = service.records.lock().await;
    let record = records.get_mut(&input.admission_id).unwrap();
    record.busy = false;
    let accepted = live
        && record.phase == "active"
        && Instant::now() < record.expires
        && outcome.as_ref().is_ok_and(|v| {
            v["request_id"] == input.request_id
                && v["admission_id"] == input.admission_id
                && v["document_generation"] == input.document_generation
        });
    let receipt = serde_json::json!({"schema":"elastos.browser.ref-input-result/v1","page_id":page_id,"request_id":input.request_id,"admission_id":input.admission_id,"document_generation":input.document_generation,"accepted":accepted,"outcome":if accepted{"completed"}else{"uncertain"}});
    record
        .receipts
        .insert(input.request_id.clone(), receipt.clone());
    if !accepted {
        record.phase = "revoked";
        if let Some(token) = &record.token {
            service
                .capabilities
                .revoke(*token.id(), "Browser input outcome uncertain")
                .await;
        }
        service.pending.revoke_request(&input.admission_id).await;
    }
    drop(records);
    if !accepted {
        let _=engine_input(&state,&owner,serde_json::json!({"type":"operator_lease","command":"release","admission_id":input.admission_id})).await;
    }
    (
        if accepted {
            StatusCode::OK
        } else {
            StatusCode::CONFLICT
        },
        Json(receipt),
    )
        .into_response()
}
