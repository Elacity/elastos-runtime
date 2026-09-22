//! An operator reads only when its owner-approved admission explicitly includes
//! inspection. Runtime projects the existing owner path internally and rechecks
//! the operator before releasing content. Detach releases a lease, not a page.
use super::*;

fn inspection_resource(page: &str, admission: &str) -> ResourceId {
    ResourceId::new(format!(
        "elastos://browser/pages/{page}/operator/{admission}/inspect"
    ))
}

pub(super) fn grant_inspection(
    service: &BrowserOperatorService,
    record: &Admission,
    id: &str,
) -> Option<CapabilityToken> {
    record.request.inspect.then(|| {
        service.capabilities.grant(
            record.session.id.as_str(),
            inspection_resource(&record.page_id, id),
            Action::Read,
            TokenConstraints::new(service.capabilities.current_epoch(), false, None, Some(16)),
            Some(SecureTimestamp::after_secs(
                u64::from(record.request.duration_ms).div_ceil(1000),
            )),
        )
    })
}

fn owner_headers(token: &str, host: &str) -> Result<HeaderMap, Box<Response>> {
    let mut headers = HeaderMap::new();
    let value = axum::http::HeaderValue::from_str(token)
        .map_err(|_| Box::new(failure(StatusCode::CONFLICT, "operator_owner_changed")))?;
    headers.insert("x-elastos-home-token", value);
    headers.insert(
        axum::http::header::HOST,
        axum::http::HeaderValue::from_str(host)
            .map_err(|_| Box::new(failure(StatusCode::CONFLICT, "operator_owner_changed")))?,
    );
    headers.insert("origin", axum::http::HeaderValue::from_static("null"));
    Ok(headers)
}

pub(in crate::api::gateway) async fn operator_inspect(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path((page_id, id)): Path<(String, String)>,
    Json(request): Json<BrowserInspectionRequest>,
) -> Response {
    if request.validate().is_err() {
        return failure(StatusCode::BAD_REQUEST, "invalid_inspection");
    }
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let session = match operator_session(&service, &headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let capability = match headers
        .get("x-elastos-capability")
        .and_then(|h| h.to_str().ok())
        .filter(|h| h.len() <= 8192)
        .and_then(|h| CapabilityToken::from_base64(h).ok())
    {
        Some(c) => c,
        None => return failure(StatusCode::UNAUTHORIZED, "operator_capability_required"),
    };
    let (owner, owner_token, owner_host, generation) = {
        let records = service.records.lock().await;
        let Some(record) = records
            .get(&id)
            .filter(|r| r.page_id == page_id && r.session.id == session.id)
        else {
            return failure(StatusCode::NOT_FOUND, "operator_admission_unavailable");
        };
        if record.phase != "active"
            || record.expires <= Instant::now()
            || !record.request.inspect
            || record
                .inspection_token
                .as_ref()
                .is_none_or(|t| t.to_base64().ok() != capability.to_base64().ok())
        {
            return failure(StatusCode::FORBIDDEN, "operator_inspection_not_granted");
        }
        if service
            .capabilities
            .validate(
                &capability,
                session.id.as_str(),
                Action::Read,
                &inspection_resource(&page_id, &id),
                None,
            )
            .await
            .is_err()
        {
            return failure(StatusCode::FORBIDDEN, "operator_capability_rejected");
        }
        let Some(owner) = record.owner.clone() else {
            return failure(StatusCode::CONFLICT, "operator_owner_changed");
        };
        (
            owner,
            record.owner_token.clone(),
            record.owner_host.clone(),
            record.request.document_generation.clone(),
        )
    };
    if !service_current(&state, &service) || !owner_live(&state, &owner, &owner_token).await {
        return failure(StatusCode::CONFLICT, "operator_owner_changed");
    }
    let owner_headers = match owner_headers(&owner_token, &owner_host) {
        Ok(h) => h,
        Err(e) => return *e,
    };
    let response = super::super::browser_page_inspection(
        state.clone(),
        owner_headers,
        page_id.clone(),
        Some(request),
    )
    .await;
    if !response.status().is_success() {
        return failure(response.status(), "operator_inspection_failed");
    }
    let body = match axum::body::to_bytes(response.into_body(), 32768).await {
        Ok(body) => body,
        Err(_) => return failure(StatusCode::BAD_GATEWAY, "operator_inspection_failed"),
    };
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return failure(StatusCode::BAD_GATEWAY, "operator_inspection_failed"),
    };
    if value["schema"] != "elastos.browser.inspect-result/v1"
        || value["page_id"] != page_id
        || value["document_generation"] != generation
    {
        return failure(StatusCode::CONFLICT, "stale_inspection");
    }
    let snapshot = service.snapshots.lock().await.get(&page_id).cloned();
    let Some(snapshot) = snapshot.filter(|s| {
        s.owner == owner
            && s.generation == generation
            && value["snapshot_id"] == s.id
            && s.expires > Instant::now()
    }) else {
        return failure(StatusCode::CONFLICT, "stale_inspection");
    };
    let mut records = service.records.lock().await;
    let Some(record) = records.get_mut(&id).filter(|r| {
        r.page_id == page_id
            && r.session.id == session.id
            && r.phase == "active"
            && r.request.inspect
            && r.expires > Instant::now()
            && r.owner.as_ref() == Some(&owner)
    }) else {
        return failure(StatusCode::FORBIDDEN, "operator_admission_inactive");
    };
    if !service_current(&state, &service)
        || !owner_live(&state, &owner, &owner_token).await
        || service
            .sessions
            .validate_token(&session.token)
            .await
            .is_none()
    {
        return failure(StatusCode::CONFLICT, "operator_owner_changed");
    }
    // The current snapshot supplies the exact refs accepted by operator_input.
    record.snapshot = Some(snapshot);
    Json(value).into_response()
}

pub(in crate::api::gateway) async fn detach_operator(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path((page_id, id)): Path<(String, String)>,
) -> Response {
    let service = match service(&state) {
        Ok(s) => s,
        Err(e) => return *e,
    };
    let session = match operator_session(&service, &headers).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let (token, host) = {
        let mut records = service.records.lock().await;
        let Some(record) = records
            .get_mut(&id)
            .filter(|r| r.page_id == page_id && r.session.id == session.id)
        else {
            return failure(StatusCode::NOT_FOUND, "operator_admission_unavailable");
        };
        // Close the disclosure gate before awaiting token or native release.
        record.phase = "revoked";
        if let Some(token) = &record.inspection_token {
            service
                .capabilities
                .revoke(*token.id(), "Browser operator detached")
                .await;
        }
        if record.owner.is_none() {
            record.phase = "revoked";
            service.pending.revoke_request(&id).await;
            return Json(
                serde_json::json!({"schema":"elastos.browser.operator-admission/v1",
                "request_id":id,"status":"revoked","writer_acquired":false}),
            )
            .into_response();
        }
        (record.owner_token.clone(), record.owner_host.clone())
    };
    let headers = match owner_headers(&token, &host) {
        Ok(h) => h,
        Err(e) => return *e,
    };
    // Reuse owner revocation, including pending native effects and release retry.
    // The operator was authenticated against the exact record above.
    revoke_admission(State(state), headers, Path((page_id, id))).await
}
