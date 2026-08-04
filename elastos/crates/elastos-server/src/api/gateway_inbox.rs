use super::*;

pub(super) async fn inbox_summary(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let authority =
        match require_runtime_wallet_authority(&state.data_dir, &headers, &[INBOX_CAPSULE_ID]) {
            Ok(authority) => authority,
            Err(err) => return inbox_error_response(err),
        };
    let context = authority.home_launch_context();

    let data_dir = state.data_dir.clone();
    let sync_context = context.clone();
    let _ = tokio::task::spawn_blocking(move || {
        home_services_sync_access_requests(&data_dir, &sync_context)
    })
    .await;

    let home_state = home_state(&state.data_dir);
    let mut notifications = home_state.notifications;
    append_home_service_access_notifications(&state.data_dir, &context, &mut notifications);
    let wallet_approvals = system_wallet_approvals_summary(&state, &authority, false).await;
    append_wallet_approval_notifications(&mut notifications, wallet_approvals.approval_requests);
    if let Ok(capability_requests) = runtime_capability_pending_requests(&state.data_dir).await {
        append_runtime_capability_notifications(&mut notifications, capability_requests);
    }
    if let Ok(inspect_requests) =
        pending_inspect_action_requests(&state.data_dir, &context.principal_id)
    {
        append_inspect_action_notifications(&mut notifications, inspect_requests);
    }
    Json(InboxSummaryResponse {
        app: HomeCapsuleIdentity {
            id: INBOX_CAPSULE_ID.to_string(),
            route: "/apps/inbox/".to_string(),
        },
        notifications,
    })
    .into_response()
}

pub(super) fn append_wallet_approval_notifications(
    notifications: &mut HomeNotificationsSummary,
    approval_requests: Vec<SystemWalletApprovalSummary>,
) {
    for request in approval_requests {
        notifications.unread_count += 1;
        notifications.attention_count += 1;
        let capsule = if request.capsule_id.trim().is_empty() {
            "A capsule".to_string()
        } else {
            request.capsule_id.clone()
        };
        notifications.entries.push(HomeNotificationEntrySummary {
            id: format!("wallet-approval-request:{}", request.request_id),
            source_app: request.capsule_id.clone(),
            kind: "wallet_approval_request".to_string(),
            title: wallet_approval_title(&request.intent),
            body: format!(
                "{} requests wallet approval for {}.",
                capsule, request.reason
            ),
            action_ref: Some(HomeNotificationActionSummary {
                app: WALLET_CAPSULE_ID.to_string(),
                action_id: format!(
                    "{}:{}",
                    if request.intent == "browser_account_access" {
                        "wallet-review-request"
                    } else {
                        "wallet-approve-request"
                    },
                    request.request_id
                ),
            }),
            severity: "attention".to_string(),
            read: false,
            created_at: request.created_at,
        });
    }
}

pub(super) async fn runtime_capability_pending_requests(
    data_dir: &std::path::Path,
) -> anyhow::Result<Vec<RuntimeCapabilityPendingRequest>> {
    let coords = load_live_runtime_coords(data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let shell_token = home_attach_shell(&client, &coords.api_url, &coords.attach_secret).await?;
    let response = client
        .get(format!("{}/api/capability/pending", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {shell_token}"))
        .send()
        .await?
        .error_for_status()?
        .json::<RuntimeCapabilityPendingResponse>()
        .await?;
    Ok(response.requests)
}

pub(super) fn append_runtime_capability_notifications(
    notifications: &mut HomeNotificationsSummary,
    capability_requests: Vec<RuntimeCapabilityPendingRequest>,
) {
    for request in capability_requests {
        notifications.unread_count += 1;
        notifications.attention_count += 1;
        notifications.entries.push(HomeNotificationEntrySummary {
            id: format!("capability-request:{}", request.request_id),
            source_app: SYSTEM_CAPSULE_ID.to_string(),
            kind: "capability_request".to_string(),
            title: capability_request_title(&request.action),
            body: format!(
                "A capsule requests {} access to {}.",
                request.action, request.resource
            ),
            action_ref: Some(HomeNotificationActionSummary {
                app: SYSTEM_CAPSULE_ID.to_string(),
                action_id: format!("capability-approve-request:{}", request.request_id),
            }),
            severity: "attention".to_string(),
            read: false,
            created_at: request.requested_at,
        });
    }
}

fn capability_request_title(action: &str) -> String {
    match action {
        "read" => "Read access request".to_string(),
        "write" => "Write access request".to_string(),
        "execute" => "Execute access request".to_string(),
        "delete" => "Delete access request".to_string(),
        "message" => "Message access request".to_string(),
        "admin" => "Admin access request".to_string(),
        _ => "Capability request".to_string(),
    }
}

fn wallet_approval_title(intent: &str) -> String {
    match intent {
        "auth_challenge" => "Wallet sign-in request".to_string(),
        "capability_grant" => "Wallet access request".to_string(),
        "credential" => "Credential signing request".to_string(),
        "publish_envelope" => "Publish approval request".to_string(),
        "transaction_intent" => "Transaction approval request".to_string(),
        "browser_account_access" => "Browser account access request".to_string(),
        "browser_personal_sign" => "Browser signature request".to_string(),
        "browser_typed_data_sign" => "Browser typed data signature request".to_string(),
        "bitcoin_bip322_proof" => "Bitcoin proof request".to_string(),
        "revocation" => "Revocation approval request".to_string(),
        _ => "Wallet approval request".to_string(),
    }
}

pub(super) async fn inbox_action(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let launch =
        match require_home_launch_token_binding(&state.data_dir, &headers, &[INBOX_CAPSULE_ID]) {
            Ok(launch) => launch,
            Err(err) => return inbox_error_response(err),
        };
    let authority = match runtime_wallet_authority(&launch) {
        Ok(authority) => authority,
        Err(err) => return inbox_error_response(err),
    };
    let context = launch.context.clone();

    let action = match parse_inbox_action_request(&headers, &body).map_err(anyhow::Error::msg) {
        Ok(req) => req,
        Err(err) => return inbox_error_response(err),
    };
    match dispatch_inbox_action(&state, &launch, &context, &authority, &action).await {
        Ok(message) => {
            let result = inspect_action_request_id_from_action(&action.action_id)
                .map(|request_id| inspect_action_result_receipt(&state.data_dir, request_id))
                .transpose();
            match result {
                Ok(result) => Json(InboxActionResponse { message, result }).into_response(),
                Err(err) => inbox_error_response(err),
            }
        }
        Err(err) => inbox_error_response(err),
    }
}

fn inspect_action_request_id_from_action(action_id: &str) -> Option<&str> {
    action_id
        .strip_prefix("inspect-approve-request:")
        .or_else(|| action_id.strip_prefix("inspect-deny-request:"))
}

fn parse_inbox_action_request(
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<InboxActionRequest, String> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("application/json") {
        serde_json::from_slice(body).map_err(|err| format!("invalid inbox action body: {err}"))
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        let action_id = form_urlencoded::parse(body.as_ref())
            .find_map(|(key, value)| (key == "action_id").then(|| value.into_owned()))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "missing action_id".to_string())?;
        Ok(InboxActionRequest {
            action_id,
            step_up_token: None,
        })
    } else {
        Err("unsupported inbox action content type".to_string())
    }
}

async fn dispatch_inbox_action(
    state: &GatewayState,
    launch: &RequiredHomeLaunchToken,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    action: &InboxActionRequest,
) -> anyhow::Result<String> {
    let action_id = action.action_id.as_str();
    let data_dir = &state.data_dir;
    if let Some(notification_id) = action_id.strip_prefix("notification-read:") {
        return Ok(
            match crate::notifications::mark_read(data_dir, notification_id)? {
                true => "Marked inbox entry read.".to_string(),
                false => "That inbox entry was already read or is no longer present.".to_string(),
            },
        );
    }
    if let Some(notification_id) = action_id.strip_prefix("notification-dismiss:") {
        return Ok(
            match crate::notifications::dismiss(data_dir, notification_id)? {
                true => "Dismissed inbox entry.".to_string(),
                false => "That inbox entry is already gone.".to_string(),
            },
        );
    }
    if let Some(request_id) = action_id.strip_prefix("room-approve-request:") {
        let message = match crate::room_service::approve_request(data_dir, request_id)? {
            Some(outcome) => format!(
                "Approved Chat web guest request for {} on {}.",
                outcome.display_name, outcome.device_label
            ),
            None => "That web guest request is no longer pending.".to_string(),
        };
        let summary = crate::room_service::load_summary(data_dir)?;
        let _ = crate::notifications::sync_room_notifications(data_dir, &summary);
        let _ = crate::notifications::mark_acted_for_action(data_dir, action_id);
        return Ok(message);
    }
    if let Some(request_id) = action_id.strip_prefix("room-deny-request:") {
        let message =
            match crate::room_service::deny_request(data_dir, request_id, "Denied from Inbox.")? {
                Some(outcome) => format!(
                    "Denied Chat web guest request for {} on {}.",
                    outcome.display_name, outcome.device_label
                ),
                None => "That web guest request is no longer pending.".to_string(),
            };
        let summary = crate::room_service::load_summary(data_dir)?;
        let _ = crate::notifications::sync_room_notifications(data_dir, &summary);
        let _ = crate::notifications::mark_acted_for_action(data_dir, action_id);
        return Ok(message);
    }
    if let Some(request_id) = action_id.strip_prefix("wallet-approve-request:") {
        let pending = pending_wallet_approval_request(state, authority, request_id).await?;
        if pending.intent == "browser_account_access" {
            anyhow::bail!("Review Browser account access in Wallet before allowing it");
        }
        let Some(step_up_token) = action.step_up_token.as_deref() else {
            anyhow::bail!("fresh passkey verification is required to sign with a built-in wallet");
        };
        consume_passkey_step_up_token(
            data_dir,
            step_up_token,
            launch,
            180,
            "wallet.approve",
            &serde_json::json!({
                "request_id": request_id,
                "reason": "Approved in Inbox",
            }),
        )?;
        let outcome = approve_pending_managed_wallet_request(
            state,
            data_dir,
            context,
            authority,
            pending,
            "Approved in Inbox",
            INBOX_CAPSULE_ID,
        )
        .await?;
        let _ = crate::notifications::mark_acted_for_action(data_dir, action_id);
        return Ok(outcome.message);
    }
    if let Some(request_id) = action_id.strip_prefix("wallet-reject-request:") {
        runtime_wallet_data(
            state,
            authority,
            elastos_wallet_contract::WalletProviderOperationV2::RejectApproval {
                request_id: request_id.to_string(),
                reason: "Rejected in Inbox".to_string(),
            },
        )
        .await?;
        append_wallet_approval_audit(
            data_dir,
            WalletApprovalAuditInput {
                capsule_id: INBOX_CAPSULE_ID,
                event_type: "wallet.approval.rejected",
                principal_id: &context.principal_id,
                session_id: &context.session_id,
                request_id,
                result: "rejected",
                reason: "Wallet approval rejected through Runtime authority",
            },
        )?;
        return Ok("Rejected wallet request.".to_string());
    }
    if let Some(request_id) = action_id.strip_prefix("capability-approve-request:") {
        return approve_runtime_capability_request(state, request_id).await;
    }
    if let Some(request_id) = action_id.strip_prefix("capability-deny-request:") {
        return deny_runtime_capability_request(state, request_id).await;
    }
    if let Some(request_id) = action_id.strip_prefix("service-approve-request:") {
        let data_dir = data_dir.clone();
        let context = context.clone();
        let request_id = request_id.to_string();
        return tokio::task::spawn_blocking(move || {
            approve_home_service_access_request(&data_dir, &context, &request_id)
        })
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    }
    if let Some(request_id) = action_id.strip_prefix("service-deny-request:") {
        let data_dir = data_dir.clone();
        let context = context.clone();
        let request_id = request_id.to_string();
        return tokio::task::spawn_blocking(move || {
            deny_home_service_access_request(&data_dir, &context, &request_id)
        })
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    }
    if let Some(request_id) = action_id.strip_prefix("inspect-approve-request:") {
        let Some(step_up_token) = action.step_up_token.as_deref() else {
            anyhow::bail!("fresh passkey verification is required to approve an Inspector action");
        };
        return approve_inspect_action_request(state, launch, context, request_id, step_up_token)
            .await;
    }
    if let Some(request_id) = action_id.strip_prefix("inspect-deny-request:") {
        return deny_inspect_action_request(state, context, request_id);
    }
    if action_id == WALLET_PRICE_HTTP_APPROVE_ACTION_ID {
        ensure_admin_context(data_dir, context)?;
        let approved_at = now_ts();
        store_wallet_price_http_policy(data_dir, &context.principal_id, approved_at)?;
        let _ = crate::notifications::mark_acted_for_action(data_dir, action_id);
        append_wallet_price_policy_audit(
            data_dir,
            &context.principal_id,
            &context.session_id,
            "approved",
            "Approved Wallet market-price HTTP source through Inbox",
        )?;
        return Ok("Approved Wallet market-price source.".to_string());
    }
    if action_id.strip_prefix(WALLET_PRICE_HTTP_DENY_ACTION_PREFIX) == Some("coingecko") {
        let _ = crate::notifications::dismiss_external_http_request(
            data_dir,
            WALLET_PRICE_HTTP_REQUEST_ID,
        );
        append_wallet_price_policy_audit(
            data_dir,
            &context.principal_id,
            &context.session_id,
            "rejected",
            "Rejected Wallet market-price HTTP source through Inbox",
        )?;
        return Ok("Rejected Wallet market-price source.".to_string());
    }
    anyhow::bail!("unknown inbox action");
}

pub(super) async fn approve_runtime_capability_request(
    state: &GatewayState,
    request_id: &str,
) -> anyhow::Result<String> {
    let data_dir = &state.data_dir;
    let agent_library = agent_library_read_job_exists(data_dir, request_id);
    let duration = if agent_library { "once" } else { "session" };
    let coords = load_live_runtime_coords(data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let shell_token = home_attach_shell(&client, &coords.api_url, &coords.attach_secret).await?;
    client
        .post(format!("{}/api/capability/grant", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {shell_token}"))
        .json(&serde_json::json!({
            "request_id": request_id,
            "duration": duration,
            "rationale": if agent_library {
                "Approved in Inbox · Agent library.read once"
            } else {
                "Approved in Inbox"
            },
        }))
        .send()
        .await?
        .error_for_status()?;
    if agent_library {
        fulfill_agent_library_read_after_grant(state, request_id).await?;
        return Ok("Approved Agent Library.read once — result ready on Home.".to_string());
    }
    Ok("Approved capsule request.".to_string())
}

pub(super) async fn deny_runtime_capability_request(
    state: &GatewayState,
    request_id: &str,
) -> anyhow::Result<String> {
    let data_dir = &state.data_dir;
    let coords = load_live_runtime_coords(data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let shell_token = home_attach_shell(&client, &coords.api_url, &coords.attach_secret).await?;
    client
        .post(format!("{}/api/capability/deny", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {shell_token}"))
        .json(&serde_json::json!({
            "request_id": request_id,
            "reason": "Denied in Inbox",
        }))
        .send()
        .await?
        .error_for_status()?;
    mark_agent_library_read_denied(data_dir, request_id);
    Ok("Denied capsule request.".to_string())
}
