use super::*;

pub(super) async fn home_launch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<HomeLaunchRequest>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let context = require_home_token_context(&state.data_dir, &headers).map_err(|err| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": err.to_string() })),
        )
    })?;

    let target = req.target.trim();
    if target.is_empty() || target == HOME_CAPSULE_ID {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid Home target" })),
        ));
    }

    let mut target_summary = home_launch_target(&state.data_dir, target);
    if target_summary.is_none() || state.data_dir.join("capsules").join(target).exists() {
        ensure_home_target_package(&state.data_dir, target, target_summary.is_none())
            .await
            .map_err(|err| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": err })),
                )
            })?;
        target_summary = home_launch_target(&state.data_dir, target);
    }

    let Some(target_summary) = target_summary else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Home target not found" })),
        ));
    };

    let launch = launch_runtime_backed_home_target(
        &state.data_dir,
        target_summary.target.as_str(),
        &context,
    )
    .await;
    let delivery = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        target_summary.target.as_str(),
        &req.query,
        &context,
        request_uses_tls(&headers),
    )
    .map_err(gateway_internal_error)?;

    let mut response = Json(HomeLaunchResponse {
        target: target_summary.target,
        title: target_summary.title,
        route: delivery.route,
        attach_kind: target_summary.attach_kind,
        role: target_summary.role,
        target_kind: target_summary.target_kind,
        launch_status: launch.as_ref().map(|summary| summary.status.clone()),
        launch_detail: launch.as_ref().and_then(|summary| summary.detail.clone()),
        capsule_id: launch.and_then(|summary| summary.capsule_id),
    })
    .into_response();
    // Cookie-delivered targets (the money surface): the launch token rides an HttpOnly Set-Cookie
    // on THIS response — the same-origin shell fetch stores it in the browser jar, and the app's
    // path-scoped API calls carry it from there. The URL above holds no credential.
    if let Some(cookie) = delivery.set_cookie {
        response.headers_mut().append(axum::http::header::SET_COOKIE, cookie);
    }
    Ok(response)
}

/// A launched app route plus, for cookie-delivered targets, the Set-Cookie header the launch
/// RESPONSE must carry. For every ordinary app `set_cookie` is `None` and the token rides the
/// URL exactly as before Sprint 33.
pub(super) struct HomeLaunchRouteDelivery {
    pub(super) route: String,
    pub(super) set_cookie: Option<HeaderValue>,
}

/// True for targets whose launch token is COOKIE-delivered instead of URL-borne. Today that is
/// exactly the mandates app — the surface carrying money verbs (Sprint 33, council S31 F1): its
/// launch URL must never contain the bearer credential (URLs are logged, copyable, and readable
/// by frame script; an HttpOnly cookie is none of those).
pub(super) fn is_cookie_delivered_target(target: &str) -> bool {
    target == super::gateway_mandates::MANDATES_CAPSULE_ID
}

pub(super) fn append_home_launch_token(
    data_dir: &std::path::Path,
    route: &str,
    target: &str,
    query: &BTreeMap<String, String>,
    context: &HomeLaunchTokenContext,
    secure: bool,
) -> anyhow::Result<HomeLaunchRouteDelivery> {
    let token = issue_home_launch_token_with_context(data_dir, target, context)?;
    let cookie_delivered = is_cookie_delivered_target(target);
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if cookie_delivered {
        // No bearer credential in the URL — only the non-secret in-shell marker the app uses to
        // distinguish "launched by the shell" from "opened standalone" (its sample mode).
        serializer.append_pair("shell", "1");
    } else {
        serializer.append_pair("home_token", &token);
    }
    for (key, value) in query {
        let key = key.trim();
        if key.is_empty() || key == "home_token" || (cookie_delivered && key == "shell") {
            continue;
        }
        serializer.append_pair(key, value);
    }
    let encoded = serializer.finish();
    let separator = if route.contains('?') { '&' } else { '?' };
    let set_cookie = if cookie_delivered {
        Some(super::gateway_home_token::app_launch_cookie_header_for_token(
            MANDATES_SESSION_COOKIE,
            &token,
            MANDATES_API_COOKIE_PATH,
            secure,
        )?)
    } else {
        None
    };
    Ok(HomeLaunchRouteDelivery {
        route: format!("{route}{separator}{encoded}"),
        set_cookie,
    })
}

pub(super) fn home_targets(data_dir: &std::path::Path) -> Vec<HomeTargetSummary> {
    let mut targets = home_browser_targets(data_dir, true);
    targets.extend(home_viewer_targets(data_dir));
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

pub(super) fn home_launch_target(
    data_dir: &std::path::Path,
    target: &str,
) -> Option<HomeTargetSummary> {
    home_browser_targets(data_dir, false)
        .into_iter()
        .chain(home_viewer_targets(data_dir))
        .find(|candidate| candidate.target == target)
}

fn home_browser_targets(data_dir: &std::path::Path, visible_only: bool) -> Vec<HomeTargetSummary> {
    let mut targets: Vec<_> =
        crate::api::browser_capsules::list_launchable_browser_capsules(data_dir)
            .into_iter()
            .filter(|app| app.name != HOME_CAPSULE_ID)
            .filter(|app| !visible_only || is_home_visible_target(&app.name))
            .map(|app| {
                let target_kind = home_target_kind(&app.name);
                HomeTargetSummary {
                    route: format!("/apps/{}/", app.name),
                    title: app_shell_title(&app.name),
                    description: app_shell_description(&app.name, app.description),
                    target: app.name,
                    attach_kind: "iframe".to_string(),
                    role: app.role,
                    target_kind,
                }
            })
            .collect();
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

fn home_viewer_targets(data_dir: &std::path::Path) -> Vec<HomeTargetSummary> {
    let mut targets = crate::api::browser_capsules::list_all_viewer_bound_capsules(data_dir)
        .into_iter()
        .map(|capsule| HomeTargetSummary {
            route: format!("/apps/{}/?capsule={}", capsule.viewer, capsule.name),
            title: viewer_object_shell_title(&capsule.name, capsule.description.as_deref()),
            description: viewer_object_shell_description(
                &capsule.viewer,
                capsule.description.as_deref(),
            ),
            target: capsule.name,
            attach_kind: "iframe".to_string(),
            role: CapsuleRole::Content,
            target_kind: HomeTargetKind::Object,
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

fn is_home_visible_target(name: &str) -> bool {
    !matches!(
        name,
        WALLET_METAMASK_CAPSULE_ID | WALLET_UNISAT_CAPSULE_ID | WALLET_WALLETCONNECT_CAPSULE_ID
    )
}

fn home_target_kind(name: &str) -> HomeTargetKind {
    match name {
        LIBRARY_CAPSULE_ID => HomeTargetKind::Object,
        _ => HomeTargetKind::App,
    }
}

pub(super) fn load_gateway_identity_summary(data_dir: &std::path::Path) -> HomeIdentitySummary {
    HomeIdentitySummary {
        device_did: load_gateway_device_did(data_dir),
        handle: None,
        profile_card: None,
    }
}

fn load_gateway_device_did(data_dir: &std::path::Path) -> Option<String> {
    let device_did = elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_, did)| did)
        .filter(|did| !did.trim().is_empty());
    device_did
}

pub(super) fn load_gateway_identity_summary_for_context(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> HomeIdentitySummary {
    let profile_card = home_profile_card_summary_for_context(data_dir, context);
    let handle = profile_card
        .as_ref()
        .map(|card| card.display_name.clone())
        .or_else(|| principal_display_name_for_context(data_dir, context));
    HomeIdentitySummary {
        device_did: load_gateway_device_did(data_dir),
        handle,
        profile_card,
    }
}

fn principal_display_name_for_context(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> Option<String> {
    let proof_binding_id = context.proof_binding_id.as_deref()?;
    crate::auth::load_principal_for_proof_binding(data_dir, proof_binding_id)
        .ok()
        .and_then(|principal| {
            let value = principal.display_name.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        })
}

pub(super) fn apply_room_access(
    summary: &mut crate::room_service::RoomSummary,
    access: crate::room_service::LocalRuntimeAccess,
) {
    summary.local_runtime_did = access.runtime_did;
    summary.local_runtime_role = access.member_role;
    summary.browser_access_allowed = access.browser_access_allowed;
    summary.browser_access_block_reason = access.block_reason;
}

pub(super) async fn launch_runtime_backed_home_target(
    data_dir: &FsPath,
    target: &str,
    context: &HomeLaunchTokenContext,
) -> Option<GatewayRuntimeLaunchOutcome> {
    if let Err(err) = ensure_home_target_package(data_dir, target, false).await {
        return Some(GatewayRuntimeLaunchOutcome {
            status: "failed".to_string(),
            capsule_id: None,
            detail: Some(err),
        });
    }

    let capsule_dir = resolve_capsule_dir(data_dir, target)?;
    let manifest = crate::api::capsule_inventory::load_capsule_manifest(&capsule_dir, target)?;
    if !manifest.role.is_shell_launchable() || manifest.capsule_type == CapsuleType::Data {
        return None;
    }

    let runtime_capsule_dir =
        match materialize_source_wasm_capsule_for_runtime(data_dir, &capsule_dir, &manifest) {
            Ok(path) => path,
            Err(err) => {
                return Some(GatewayRuntimeLaunchOutcome {
                    status: "failed".to_string(),
                    capsule_id: None,
                    detail: Some(err.to_string()),
                });
            }
        };

    if let Err(err) = crate::runtime_control::ensure_runtime_for_home(data_dir).await {
        return Some(GatewayRuntimeLaunchOutcome {
            status: "failed".to_string(),
            capsule_id: None,
            detail: Some(format!("managed local runtime could not start: {err}")),
        });
    }

    Some(
        match launch_runtime_capsule(data_dir, &runtime_capsule_dir, &manifest.name, context).await
        {
            Ok(outcome) => outcome,
            Err(err) => GatewayRuntimeLaunchOutcome {
                status: "failed".to_string(),
                capsule_id: None,
                detail: Some(err.to_string()),
            },
        },
    )
}

fn materialize_source_wasm_capsule_for_runtime(
    data_dir: &FsPath,
    capsule_dir: &FsPath,
    manifest: &elastos_common::CapsuleManifest,
) -> anyhow::Result<PathBuf> {
    let entrypoint = capsule_dir.join(&manifest.entrypoint);
    if entrypoint.is_file() || manifest.capsule_type != CapsuleType::Wasm {
        return Ok(capsule_dir.to_path_buf());
    }

    let built_entrypoint = capsule_dir
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join(&manifest.entrypoint);
    if !built_entrypoint.is_file() {
        anyhow::bail!(
            "capsule runtime entrypoint missing: {}",
            capsule_dir.join(&manifest.entrypoint).display()
        );
    }

    let bundle_dir = data_dir.join("dev-capsules").join(&manifest.name);
    std::fs::create_dir_all(&bundle_dir)?;
    std::fs::copy(
        capsule_dir.join("capsule.json"),
        bundle_dir.join("capsule.json"),
    )?;
    std::fs::copy(&built_entrypoint, bundle_dir.join(&manifest.entrypoint))?;
    Ok(bundle_dir)
}

async fn launch_runtime_capsule(
    data_dir: &FsPath,
    capsule_dir: &FsPath,
    capsule_name: &str,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<GatewayRuntimeLaunchOutcome> {
    let coords = load_live_runtime_coords(data_dir)
        .await
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let home_token =
        gateway_attach_runtime_token(&client, &coords.api_url, &coords.attach_secret, "shell")
            .await?;
    let response = client
        .post(format!("{}/api/capsules", coords.api_url))
        .header(AUTHORIZATION, format!("Bearer {home_token}"))
        .json(&serde_json::json!({
            "path": capsule_dir.display().to_string(),
            "launch_grant": issue_home_launch_token_with_context(data_dir, capsule_name, context)?,
        }))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("runtime launch failed ({}): {}", status, text.trim());
    }
    let payload = response.json::<GatewayRuntimeLaunchResponse>().await?;
    Ok(GatewayRuntimeLaunchOutcome {
        status: "launched".to_string(),
        capsule_id: Some(payload.id),
        detail: None,
    })
}

async fn ensure_home_target_package(
    data_dir: &FsPath,
    target: &str,
    required: bool,
) -> Result<(), String> {
    if !required && !crate::setup::capsule_component_has_release_identity(data_dir, target) {
        return Ok(());
    }

    crate::setup::ensure_capsule_component_for_home_launch(data_dir, target)
        .await
        .map(|_| ())
        .map_err(|err| format!("Home target package materialization failed: {err}"))
}

pub(super) async fn system_runtime_log(data_dir: &FsPath) -> SystemRuntimeLogSummary {
    let Some(coords) = load_live_runtime_coords(data_dir).await else {
        return SystemRuntimeLogSummary {
            available: false,
            total_in_memory: None,
            current_epoch: None,
            events: Vec::new(),
            note: Some("Local runtime is not running.".to_string()),
        };
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime client unavailable: {err}")),
            }
        }
    };

    let home_token = match gateway_attach_runtime_token(
        &client,
        &coords.api_url,
        &coords.attach_secret,
        "shell",
    )
    .await
    {
        Ok(token) => token,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime attach failed: {err}")),
            }
        }
    };

    let response = match client
        .get(format!(
            "{}/api/audit?limit={}",
            coords.api_url, SYSTEM_RUNTIME_ACTIVITY_FETCH_LIMIT
        ))
        .header(AUTHORIZATION, format!("Bearer {home_token}"))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime log unavailable: {err}")),
            }
        }
    };

    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return SystemRuntimeLogSummary {
            available: false,
            total_in_memory: None,
            current_epoch: None,
            events: Vec::new(),
            note: Some(format!(
                "Runtime log unavailable ({}): {}",
                status,
                text.trim()
            )),
        };
    }

    let payload = match response.json::<GatewayAuditLogResponse>().await {
        Ok(payload) => payload,
        Err(err) => {
            return SystemRuntimeLogSummary {
                available: false,
                total_in_memory: None,
                current_epoch: None,
                events: Vec::new(),
                note: Some(format!("Runtime log could not be decoded: {err}")),
            }
        }
    };

    SystemRuntimeLogSummary {
        available: true,
        total_in_memory: Some(payload.total_in_memory),
        current_epoch: Some(payload.current_epoch),
        events: system_runtime_activity_summaries(payload.events),
        note: None,
    }
}

pub(super) fn system_runtime_activity_summaries(
    events: Vec<elastos_runtime::primitives::audit::AuditEvent>,
) -> Vec<SystemRuntimeEventSummary> {
    let mut summaries = events
        .into_iter()
        .filter_map(system_runtime_event_summary)
        .collect::<Vec<_>>();
    summaries.sort_by_key(|event| std::cmp::Reverse(event.at.unwrap_or_default()));
    summaries.truncate(SYSTEM_RUNTIME_ACTIVITY_DISPLAY_LIMIT);
    summaries
}

fn system_runtime_event_summary(
    event: elastos_runtime::primitives::audit::AuditEvent,
) -> Option<SystemRuntimeEventSummary> {
    use elastos_runtime::primitives::audit::{AuditEvent, StopReason};

    let kind = event.event_type_name().to_string();
    let at = match &event {
        AuditEvent::RuntimeStart { timestamp, .. }
        | AuditEvent::RuntimeStop { timestamp }
        | AuditEvent::CapsuleLaunch { timestamp, .. }
        | AuditEvent::CapsuleStop { timestamp, .. }
        | AuditEvent::CapabilityGrant { timestamp, .. }
        | AuditEvent::CapabilityRevoke { timestamp, .. }
        | AuditEvent::CapabilityUse { timestamp, .. }
        | AuditEvent::ContentFetch { timestamp, .. }
        | AuditEvent::ContentOpen { timestamp, .. }
        | AuditEvent::AuthAttempt { timestamp, .. }
        | AuditEvent::EpochAdvance { timestamp, .. }
        | AuditEvent::ConfigChange { timestamp, .. }
        | AuditEvent::SecurityWarning { timestamp, .. }
        | AuditEvent::SessionCreated { timestamp, .. }
        | AuditEvent::SessionDestroyed { timestamp, .. }
        | AuditEvent::CapabilityRequested { timestamp, .. }
        | AuditEvent::CapabilityDenied { timestamp, .. }
        | AuditEvent::CapabilityApproved { timestamp, .. }
        | AuditEvent::SpendDebit { timestamp, .. }
        | AuditEvent::BudgetExhausted { timestamp, .. }
        | AuditEvent::EgressDenied { timestamp, .. }
        | AuditEvent::IntentDeclared { timestamp, .. }
        | AuditEvent::IntentDenied { timestamp, .. }
        | AuditEvent::IntentReconciled { timestamp, .. }
        | AuditEvent::IdentityRegistered { timestamp, .. }
        | AuditEvent::StorageAccess { timestamp, .. }
        | AuditEvent::MessageSent { timestamp, .. }
        | AuditEvent::PolicyProposal { timestamp, .. }
        | AuditEvent::PolicyDecisionMade { timestamp, .. }
        | AuditEvent::PolicyDivergence { timestamp, .. } => Some(timestamp.unix_secs),
        AuditEvent::Custom { .. } => None,
    };

    let summary = match event {
        AuditEvent::RuntimeStart { version, .. } => format!("Runtime started ({version})"),
        AuditEvent::RuntimeStop { .. } => "Runtime stopped".to_string(),
        AuditEvent::CapsuleLaunch { capsule_name, .. } => {
            format!("Opened {capsule_name}")
        }
        AuditEvent::CapsuleStop {
            capsule_id, reason, ..
        } => match reason {
            StopReason::Requested | StopReason::Completed => {
                format!("Stopped {capsule_id}")
            }
            StopReason::Error(detail) => format!("Stopped {capsule_id} — error: {detail}"),
            StopReason::ResourceLimit(detail) => {
                format!("Stopped {capsule_id} — resource limit: {detail}")
            }
            StopReason::SecurityViolation(detail) => {
                format!("Stopped {capsule_id} — security violation: {detail}")
            }
        },
        AuditEvent::CapabilityGrant { .. } => return None,
        AuditEvent::CapabilityRevoke { reason, .. } => format!("Capability revoked — {reason}"),
        AuditEvent::CapabilityUse { .. } => return None,
        AuditEvent::ContentFetch { cid, success, .. } => {
            if success {
                return None;
            }
            format!("Content fetch failed — {cid}")
        }
        AuditEvent::ContentOpen {
            content_id,
            decision,
            ..
        } => format!("Content {decision} — {content_id}"),
        AuditEvent::AuthAttempt {
            identity,
            success,
            method,
            ..
        } => {
            if success {
                return None;
            }
            format!("Authentication failed for {identity} via {method}")
        }
        AuditEvent::EpochAdvance {
            new_epoch, reason, ..
        } => format!("Capability epoch advanced to {new_epoch} — {reason}"),
        AuditEvent::ConfigChange { setting, .. } => format!("Changed {setting}"),
        AuditEvent::SecurityWarning {
            warning_type,
            details,
            ..
        } => format!("Security warning — {warning_type}: {details}"),
        AuditEvent::SessionCreated { .. } => return None,
        AuditEvent::SessionDestroyed { .. } => return None,
        AuditEvent::CapabilityRequested { .. } => return None,
        AuditEvent::CapabilityDenied { reason, .. } => format!("Capability denied — {reason}"),
        AuditEvent::CapabilityApproved {
            action, resource, ..
        } => format!("Capability approved — {action} {resource}"),
        // Per-act debits are too frequent for the ribbon; an EXHAUSTION (a refused act) is notable.
        AuditEvent::SpendDebit { .. } => return None,
        AuditEvent::BudgetExhausted {
            capsule_id,
            operation,
            ..
        } => format!("Spend budget exhausted — {capsule_id} ({operation})"),
        // A contained egress attempt is a notable custody moment for the ribbon.
        AuditEvent::EgressDenied {
            capsule_id, dest, ..
        } => format!("Egress blocked — {capsule_id} → {dest}"),
        // The routine "before" of every act (intent-proof loop) — too frequent for the ribbon.
        AuditEvent::IntentDeclared { .. } => return None,
        // A refused act (intent ⊄ envelope) is a notable containment moment.
        AuditEvent::IntentDenied {
            capsule_id,
            method_id,
            reason,
            ..
        } => format!("Intent denied — {capsule_id} {method_id} ({reason})"),
        // Only a DIVERGED / UNDELIVERED verdict is notable; a clean match is routine.
        AuditEvent::IntentReconciled {
            capsule_id,
            status,
            divergence_detail,
            ..
        } => {
            if status == "matched" {
                return None;
            }
            if divergence_detail.is_empty() {
                format!("Intent {status} — {capsule_id}")
            } else {
                format!("Intent {status} — {capsule_id} ({divergence_detail})")
            }
        }
        AuditEvent::IdentityRegistered {
            user_id, method, ..
        } => format!("Registered identity {user_id} via {method}"),
        AuditEvent::StorageAccess {
            uri,
            action,
            success,
            ..
        } => {
            if success {
                return None;
            }
            format!("Storage access failed — {action} {uri}")
        }
        AuditEvent::MessageSent { .. } => return None,
        AuditEvent::PolicyProposal { .. } => return None,
        AuditEvent::PolicyDecisionMade { .. } => return None,
        AuditEvent::PolicyDivergence {
            real_outcome,
            shadow_outcome,
            ..
        } => format!("Policy divergence — real {real_outcome}, shadow {shadow_outcome}"),
        AuditEvent::Custom { event_type, .. } => format!("Custom event — {event_type}"),
    };

    Some(SystemRuntimeEventSummary { kind, at, summary })
}

pub(super) fn resolve_capsule_dir(data_dir: &FsPath, app: &str) -> Option<PathBuf> {
    for candidate in crate::api::capsule_inventory::capsule_dir_candidates(data_dir, app) {
        if let Some(manifest) =
            crate::api::capsule_inventory::load_capsule_manifest(&candidate, app)
        {
            if manifest.name == app {
                return Some(candidate);
            }
        }
    }
    None
}

pub(super) fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn title_case_capsule_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn is_wallet_connector_capsule_id(name: &str) -> bool {
    WALLET_CONNECTOR_CAPSULE_IDS.contains(&name)
}

pub(super) fn wallet_connector_label(name: &str) -> &'static str {
    match name {
        WALLET_METAMASK_CAPSULE_ID => "MetaMask",
        WALLET_UNISAT_CAPSULE_ID => "UniSat",
        WALLET_WALLETCONNECT_CAPSULE_ID => "WalletConnect",
        _ => "Wallet",
    }
}

pub(super) fn wallet_connector_evm_chains() -> serde_json::Value {
    serde_json::json!([
        {
            "chainId": "0x14",
            "chainName": "Elastos Smart Chain",
            "nativeCurrency": {"name": "ELA", "symbol": "ELA", "decimals": 18},
            "rpcUrls": ["https://api.elastos.io/esc"],
        },
        {
            "chainId": "0x2105",
            "chainName": "Base",
            "nativeCurrency": {"name": "Ether", "symbol": "ETH", "decimals": 18},
            "rpcUrls": ["https://mainnet.base.org"],
        },
    ])
}

fn app_shell_title(name: &str) -> String {
    match name {
        DOCUMENTS_CAPSULE_ID => "Documents".to_string(),
        CHAT_ROOM_CAPSULE_ID => "Chat".to_string(),
        LIBRARY_CAPSULE_ID => "Library".to_string(),
        MARKETPLACE_CAPSULE_ID => "Marketplace".to_string(),
        INBOX_CAPSULE_ID => "Inbox".to_string(),
        SERVICES_CAPSULE_ID => "Services".to_string(),
        SYSTEM_CAPSULE_ID => "System".to_string(),
        BROWSER_CAPSULE_ID => "Browser".to_string(),
        WALLET_CAPSULE_ID => "Wallet".to_string(),
        "archive-manager" => "Archive".to_string(),
        "gba-emulator" => "GBA Emulator".to_string(),
        "elacity-player" => "Owned Video".to_string(),
        "ddrm-viewer" => "Owned Asset".to_string(),
        _ if is_wallet_connector_capsule_id(name) => wallet_connector_label(name).to_string(),
        _ => title_case_capsule_name(name),
    }
}

fn app_shell_description(name: &str, manifest_description: Option<String>) -> String {
    match name {
        DOCUMENTS_CAPSULE_ID => {
            "Create, edit, and publish markdown documents from this device.".to_string()
        }
        CHAT_ROOM_CAPSULE_ID => "Open Chat conversations inside ElastOS.".to_string(),
        LIBRARY_CAPSULE_ID => "Browse documents and open them in Documents.".to_string(),
        MARKETPLACE_CAPSULE_ID => {
            "Browse installed capsules, providers, viewers, and content.".to_string()
        }
        INBOX_CAPSULE_ID => "Review requests and approvals for this Home.".to_string(),
        SERVICES_CAPSULE_ID => "Manage Browser Exit Node sharing and subscriptions.".to_string(),
        SYSTEM_CAPSULE_ID => {
            "Manage passkeys, appearance, and runtime settings for this Home.".to_string()
        }
        BROWSER_CAPSULE_ID => "Open web sites through the ElastOS Browser boundary.".to_string(),
        WALLET_CAPSULE_ID => {
            "View accounts, balances, approvals, and approval methods.".to_string()
        }
        "archive-manager" => "Open archives selected from Library.".to_string(),
        _ if is_wallet_connector_capsule_id(name) => format!(
            "Add {} as an approval method.",
            wallet_connector_label(name)
        ),
        "gba-emulator" => "Launch the browser-based mGBA frontend.".to_string(),
        "elacity-player" => {
            "Play an owned, protected video end-to-end through the local dDRM decrypt boundary."
                .to_string()
        }
        "ddrm-viewer" => {
            "Open an owned, protected asset (image, document, 3D) through the local dDRM decrypt boundary."
                .to_string()
        }
        _ => manifest_description
            .unwrap_or_else(|| format!("Open {} from Home.", app_shell_title(name))),
    }
}

pub(crate) fn viewer_object_shell_title(name: &str, description: Option<&str>) -> String {
    if name == "archive-manager" {
        return "Archive".to_string();
    }
    let Some(description) = description.map(str::trim).filter(|value| !value.is_empty()) else {
        return title_case_capsule_name(name);
    };
    for separator in [" - ", " — ", ": "] {
        if let Some((title, _)) = description.split_once(separator) {
            let title = title.trim();
            if !title.is_empty() && title.len() <= 48 {
                return title.to_string();
            }
        }
    }
    title_case_capsule_name(name)
}

pub(crate) fn viewer_object_shell_description(viewer: &str, description: Option<&str>) -> String {
    description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Open this object in {}.", app_shell_title(viewer)))
}

pub(super) fn home_state(data_dir: &std::path::Path) -> HomeState {
    let site = home_site_summary(data_dir);

    let identity = room_service_runtime_identity_profile(data_dir);
    let mut room_summary = crate::room_service::load_summary(data_dir).unwrap_or_default();
    if let Ok(hosted) = crate::browser_app_hosts::load_browser_app_hosted_endpoint(
        data_dir,
        crate::room_service::room_slug(),
    ) {
        room_summary.canonical_hosted_guest_url = hosted.canonical_url;
        room_summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
    }
    if let Ok(access) = crate::room_service::local_runtime_access(data_dir, identity.did.as_deref())
    {
        apply_room_access(&mut room_summary, access);
    }
    let people = home_people_summary(&room_summary, identity.did.as_deref());
    let services = home_services_summary(data_dir, &room_summary, &people);
    let _ = crate::notifications::sync_room_notifications(data_dir, &room_summary);
    let notifications = crate::notifications::load_summary(data_dir).unwrap_or_default();

    HomeState {
        site,
        room: home_room_summary(room_summary),
        people,
        services,
        notifications: home_notifications_summary(notifications),
    }
}

fn home_site_summary(data_dir: &std::path::Path) -> HomeSiteSummary {
    let site_root = my_website_root_path(data_dir);
    let active_head = std::fs::read(edge_site_head_path(data_dir, MY_WEBSITE_URI))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SiteHeadEnvelope>(&bytes).ok());
    let release_count = std::fs::read_dir(publisher_site_releases_dir(data_dir, MY_WEBSITE_URI))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .count();

    HomeSiteSummary {
        staged: site_root.join("index.html").exists(),
        root_uri: MY_WEBSITE_URI.to_string(),
        path: site_root.display().to_string(),
        active_release: active_head
            .as_ref()
            .and_then(|head| head.payload.release_name.clone()),
        active_channel: active_head
            .as_ref()
            .and_then(|head| head.payload.channel_name.clone()),
        active_bundle_cid: active_head
            .as_ref()
            .and_then(|head| head.payload.bundle_cid.clone()),
        release_count,
    }
}

fn home_room_summary(summary: crate::room_service::RoomSummary) -> HomeRoomSummary {
    HomeRoomSummary {
        room_slug: summary.room_slug,
        title: summary.room_control.title,
        member_count: summary.room_control.member_count,
        active_member_count: summary.room_control.active_member_count,
        pending_count: summary.pending_count,
        active_session_count: summary.active_session_count,
        latest_request_name: summary.latest_request_name,
        latest_request_device: summary.latest_request_device,
        local_runtime_did: summary.local_runtime_did,
        local_runtime_role: summary.local_runtime_role.map(home_room_role_label),
        canonical_hosted_guest_url: summary.canonical_hosted_guest_url,
        ephemeral_hosted_guest_url: summary.ephemeral_hosted_guest_url,
        browser_access_allowed: summary.browser_access_allowed,
        browser_access_block_reason: summary.browser_access_block_reason,
        pending_requests: summary
            .pending_requests
            .into_iter()
            .map(|request| HomePendingRequestSummary {
                request_id: request.request_id,
                display_name: request.display_name,
                device_label: request.device_label,
                requested_at: request.requested_at,
            })
            .collect(),
        active_sessions: summary
            .active_sessions
            .into_iter()
            .map(|session| HomeActiveSessionSummary {
                display_name: session.display_name,
                device_label: session.device_label,
                approved_at: session.approved_at,
                last_seen_at: session.last_seen_at,
            })
            .collect(),
    }
}

fn home_people_summary(
    summary: &crate::room_service::RoomSummary,
    local_did: Option<&str>,
) -> HomePeopleSummary {
    let active_by_member = summary
        .active_participants
        .iter()
        .filter_map(|participant| {
            participant
                .member_did
                .as_deref()
                .map(|member_did| (member_did.to_string(), participant))
        })
        .collect::<BTreeMap<_, _>>();
    let mut contacts = summary
        .room_control
        .members
        .iter()
        .filter(|member| local_did != Some(member.member_did.as_str()))
        .enumerate()
        .map(|(index, member)| {
            let active = active_by_member.get(&member.member_did);
            let profile_card = member
                .profile_card
                .clone()
                .map(home_profile_card_from_room_profile);
            let display_name = profile_card
                .as_ref()
                .map(|card| card.display_name.clone())
                .or_else(|| active.map(|participant| participant.display_name.clone()))
                .unwrap_or_else(|| home_people_fallback_display_name(&member.role, index));
            HomePeopleContactSummary {
                contact_id: home_people_contact_id(&member.member_did),
                added_at: member.added_at,
                display_name,
                handle: profile_card.as_ref().and_then(|card| card.handle.clone()),
                relationship: "conversation".to_string(),
                route: format!("/apps/{CHAT_ROOM_CAPSULE_ID}/"),
                can_message: true,
                device_label: active.map(|participant| participant.device_label.clone()),
                profile_card,
                last_seen_at: active.map(|participant| participant.last_seen_at),
            }
        })
        .collect::<Vec<_>>();
    contacts.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.contact_id.cmp(&right.contact_id))
    });
    let service_offers = contacts
        .iter()
        .flat_map(home_service_offers_for_people_contact)
        .collect::<Vec<_>>();
    HomePeopleSummary {
        contact_count: contacts.len(),
        service_offer_count: service_offers.len(),
        contacts,
        service_offers,
        ..HomePeopleSummary::default()
    }
}

pub(super) fn home_service_offers_for_people_contact(
    contact: &HomePeopleContactSummary,
) -> Vec<HomeServiceOfferSummary> {
    let mut offers = Vec::new();
    if contact.can_message {
        offers.push(HomeServiceOfferSummary {
            schema: "elastos.service.offer/v1".to_string(),
            offer_id: format!("offer:{}:conversation", contact.contact_id),
            service_uri: "elastos://peer/conversation".to_string(),
            service_kind: "conversation".to_string(),
            display_name: format!("Chat with {}", contact.display_name),
            provider_uri: Some("elastos://carrier/conversation".to_string()),
            provider_label: "Conversation provider".to_string(),
            policy_summary: "Enabled for this accepted contact; Runtime keeps message transport behind the Chat capability.".to_string(),
            status: "enabled".to_string(),
            enabled: true,
            grant_required: false,
            grant_scope: "chat_room_contact".to_string(),
            capsule_contract: "chat-room -> Home launch grant -> room.access -> conversation provider".to_string(),
            source: "people_contact".to_string(),
            runtime_contract: None,
            contact_id: Some(contact.contact_id.clone()),
            capsule_hint: Some(CHAT_ROOM_CAPSULE_ID.to_string()),
            route: Some(format!("/apps/{CHAT_ROOM_CAPSULE_ID}/")),
        });
    }
    offers.push(HomeServiceOfferSummary {
        schema: "elastos.service.offer/v1".to_string(),
        offer_id: format!("offer:{}:browser-exit", contact.contact_id),
        service_uri: "elastos://peer/browser-exit".to_string(),
        service_kind: "remote_exit".to_string(),
        display_name: format!("{}'s Browser Exit", contact.display_name),
        provider_uri: Some("elastos://exit/remote-carrier".to_string()),
        provider_label: "Remote Exit".to_string(),
        policy_summary: "Request access from this person; a principal-scoped remote Exit grant must be installed before Browser can use it.".to_string(),
        status: "requestable".to_string(),
        enabled: false,
        grant_required: true,
        grant_scope: "principal_scoped_remote_exit_grant".to_string(),
        capsule_contract: "browser -> runtime capability -> remote Exit grant -> provider".to_string(),
        source: "people_contact".to_string(),
        runtime_contract: None,
        contact_id: Some(contact.contact_id.clone()),
        capsule_hint: Some("browser".to_string()),
        route: None,
    });
    offers
}

fn home_services_summary(
    data_dir: &std::path::Path,
    room_summary: &crate::room_service::RoomSummary,
    people: &HomePeopleSummary,
) -> HomeServicesSummary {
    let mut local_offers = home_local_service_offers(data_dir, room_summary);
    local_offers.sort_by(|left, right| {
        left.service_kind
            .cmp(&right.service_kind)
            .then_with(|| left.offer_id.cmp(&right.offer_id))
    });
    let mut remote_offers = people.service_offers.clone();
    remote_offers.extend(home_configured_remote_exit_offers(data_dir));
    remote_offers.sort_by(|left, right| {
        left.service_kind
            .cmp(&right.service_kind)
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.offer_id.cmp(&right.offer_id))
    });
    HomeServicesSummary {
        local_offer_count: local_offers.len(),
        remote_offer_count: remote_offers.len(),
        local_offers,
        remote_offers,
        ..HomeServicesSummary::default()
    }
}

pub(super) fn home_configured_remote_exit_offers(
    data_dir: &std::path::Path,
) -> Vec<HomeServiceOfferSummary> {
    let config_path = data_dir.join("config/exit-provider.json");
    let Some(exits) = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|config| {
            config
                .get("remote_carrier_exits")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
    else {
        return Vec::new();
    };
    exits
        .iter()
        .enumerate()
        .filter_map(home_configured_remote_exit_offer)
        .collect()
}

fn home_configured_remote_exit_offer(
    (index, exit): (usize, &serde_json::Value),
) -> Option<HomeServiceOfferSummary> {
    let id = home_configured_remote_exit_id(exit, index)?;
    let display_name = home_configured_remote_exit_display_name(&id);
    Some(HomeServiceOfferSummary {
        schema: "elastos.service.offer/v1".to_string(),
        offer_id: format!("configured:remote-exit:{id}"),
        service_uri: "elastos://exit/remote-carrier".to_string(),
        service_kind: "remote_exit".to_string(),
        display_name,
        provider_uri: Some("elastos://exit/remote-carrier".to_string()),
        provider_label: "Remote Exit".to_string(),
        policy_summary: "Installed remote Exit grant. Browser can use it through Runtime without exposing private route tickets to apps.".to_string(),
        status: "active".to_string(),
        enabled: true,
        grant_required: false,
        grant_scope: "installed_remote_carrier_exit_grant".to_string(),
        capsule_contract: "browser -> runtime capability -> installed remote Exit grant -> provider".to_string(),
        source: "configured_remote_exit".to_string(),
        runtime_contract: None,
        contact_id: None,
        capsule_hint: Some("browser".to_string()),
        route: Some("/apps/browser/".to_string()),
    })
}

fn home_configured_remote_exit_id(exit: &serde_json::Value, index: usize) -> Option<String> {
    let raw = exit
        .get("id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| exit.get("grant_id").and_then(serde_json::Value::as_str))
        .unwrap_or("remote-carrier-exit");
    let sanitized = raw
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ':' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('-').trim();
    if sanitized.is_empty() || sanitized.len() > 128 {
        return Some(format!("remote-carrier-exit-{index}"));
    }
    Some(sanitized.to_string())
}

fn home_configured_remote_exit_display_name(id: &str) -> String {
    let label = id
        .trim()
        .replace(['-', '_', ':'], " ")
        .split_whitespace()
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if label.is_empty() {
        "Remote Browser Exit".to_string()
    } else {
        format!("{label} Browser Exit")
    }
}

fn home_browser_engine_runtime_contract(
    data_dir: &std::path::Path,
) -> HomeServiceRuntimeContractSummary {
    let mut display_modes = std::collections::BTreeSet::new();
    let mut guarantee_levels = std::collections::BTreeSet::new();
    let mut has_microvm = false;
    let mut has_remote_operator_vm = false;
    let mut has_policy_webview = false;
    let mut has_operator_rbi = false;
    let config_path = data_dir.join("config/browser-engine-adapter.json");

    let config = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    if let Some(adapters) = config
        .as_ref()
        .and_then(|value| value.get("adapters"))
        .and_then(|value| value.as_array())
    {
        for adapter in adapters {
            if let Some(modes) = adapter
                .get("display_modes")
                .and_then(|value| value.as_array())
            {
                for mode in modes.iter().filter_map(|value| value.as_str()) {
                    if home_browser_engine_supported_display_mode(mode) {
                        display_modes.insert(mode.to_string());
                    }
                }
            }
            let kind = adapter.get("kind").and_then(|value| value.as_str());
            match kind {
                Some("chromium_microvm") => {
                    has_microvm = true;
                    guarantee_levels.insert("mechanism_microvm".to_string());
                    if browser_engine_adapter_uses_remote_vm_launcher(adapter) {
                        has_remote_operator_vm = true;
                    }
                }
                Some("cef") | Some("webview2") | Some("geckoview") | Some("wkwebview") => {
                    has_policy_webview = true;
                    guarantee_levels.insert("policy_webview".to_string());
                }
                Some("selkies_gstreamer")
                | Some("hosted_remote_browser")
                | Some("chromium_headless")
                | Some("contract_proof") => {
                    has_operator_rbi = true;
                    guarantee_levels.insert("operator_rbi".to_string());
                }
                _ => {}
            }
        }
    }

    let backing_substrate = if has_remote_operator_vm {
        "remote_operator_vm"
    } else if has_microvm {
        "local_microvm"
    } else if has_policy_webview {
        "host_policy_webview"
    } else if has_operator_rbi {
        "operator_rbi"
    } else if config_path.is_file() {
        "configured_provider"
    } else {
        "installed_provider"
    };

    HomeServiceRuntimeContractSummary {
        schema: "elastos.service.runtime-contract/v1".to_string(),
        backing_substrate: backing_substrate.to_string(),
        supported_display_modes: display_modes.into_iter().collect(),
        supported_guarantee_levels: guarantee_levels.into_iter().collect(),
        direct_network: false,
        wallet_injection: false,
    }
}

fn home_browser_engine_supported_display_mode(mode: &str) -> bool {
    matches!(mode, "webrtc_remote_display" | "native_surface")
}

fn browser_engine_adapter_uses_remote_vm_launcher(adapter: &serde_json::Value) -> bool {
    let launcher = adapter
        .get("supervisor")
        .and_then(|value| value.get("env"))
        .and_then(|value| value.get("ELASTOS_BROWSER_VM_CONTROL_LAUNCHER"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            adapter
                .get("supervisor")
                .and_then(|value| value.get("program"))
                .and_then(|value| value.as_str())
        })
        .unwrap_or_default();
    std::path::Path::new(launcher)
        .file_name()
        .and_then(|value| value.to_str())
        .map(|name| name.starts_with("browser-vm-remote-vz-launcher"))
        .unwrap_or(false)
}

fn home_local_service_offers(
    data_dir: &std::path::Path,
    room_summary: &crate::room_service::RoomSummary,
) -> Vec<HomeServiceOfferSummary> {
    let mut offers = Vec::new();
    let conversation_status = if room_summary.local_runtime_did.is_none() {
        "identity_required"
    } else if room_summary.browser_access_allowed {
        "available"
    } else {
        "blocked"
    };
    let conversation_policy = room_summary
        .browser_access_block_reason
        .clone()
        .unwrap_or_else(|| "Your Runtime can host trusted People conversations; guests still need invite approval and Chat capability.".to_string());
    offers.push(HomeServiceOfferSummary {
        schema: "elastos.service.offer/v1".to_string(),
        offer_id: "local:carrier:conversation-host".to_string(),
        service_uri: "elastos://carrier/conversation".to_string(),
        service_kind: "conversation_host".to_string(),
        display_name: "Conversation hosting".to_string(),
        provider_uri: Some("elastos://carrier/room".to_string()),
        provider_label: "Conversation provider".to_string(),
        policy_summary: conversation_policy,
        status: conversation_status.to_string(),
        enabled: room_summary.local_runtime_did.is_some() && room_summary.browser_access_allowed,
        grant_required: true,
        grant_scope: "room.access".to_string(),
        capsule_contract: "chat-room -> room.access -> conversation provider".to_string(),
        source: "local_runtime".to_string(),
        runtime_contract: None,
        contact_id: None,
        capsule_hint: Some(CHAT_ROOM_CAPSULE_ID.to_string()),
        route: Some(format!("/apps/{CHAT_ROOM_CAPSULE_ID}/")),
    });
    if data_dir.join("config/exit-provider.json").is_file() {
        offers.push(HomeServiceOfferSummary {
            schema: "elastos.service.offer/v1".to_string(),
            offer_id: "local:provider:browser-exit".to_string(),
            service_uri: "elastos://exit/browser".to_string(),
            service_kind: "remote_exit".to_string(),
            display_name: "Browser Exit node".to_string(),
            provider_uri: Some("elastos://exit/*".to_string()),
            provider_label: "Exit provider".to_string(),
            policy_summary: "Can be offered to trusted people; Exit provider enforces destination policy, quotas, audit, and principal-scoped grants.".to_string(),
            status: "configured".to_string(),
            enabled: true,
            grant_required: true,
            grant_scope: "principal_scoped_exit_grant".to_string(),
            capsule_contract: "browser -> net capability -> exit grant -> Exit provider".to_string(),
            source: "local_provider".to_string(),
            runtime_contract: None,
            contact_id: None,
            capsule_hint: Some("browser".to_string()),
            route: None,
        });
    }
    if data_dir
        .join("config/browser-engine-adapter.json")
        .is_file()
        || data_dir.join("bin/browser-engine-adapter").is_file()
    {
        let runtime_contract = home_browser_engine_runtime_contract(data_dir);
        let policy_summary = match runtime_contract.backing_substrate.as_str() {
            "remote_operator_vm" => "Delegates Browser pages to an approved remote VM provider through a local Runtime-facing control socket; provider enforces display/input/stream receipts and no direct network or wallet authority.",
            "local_microvm" => "Runs isolated Browser pages through a local VM substrate and Runtime-mediated Exit streams; provider enforces display/input/stream receipts and no direct network or wallet authority.",
            _ => "Runs Browser pages only through the Browser Engine Adapter contract; provider must prove display/input/stream receipts and no direct network or wallet authority.",
        };
        offers.push(HomeServiceOfferSummary {
            schema: "elastos.service.offer/v1".to_string(),
            offer_id: "local:provider:browser-engine".to_string(),
            service_uri: "elastos://browser-engine/launch".to_string(),
            service_kind: "browser_engine".to_string(),
            display_name: "Browser Engine".to_string(),
            provider_uri: Some("elastos://browser-engine/*".to_string()),
            provider_label: "Browser Engine Adapter".to_string(),
            policy_summary: policy_summary.to_string(),
            status: "configured".to_string(),
            enabled: true,
            grant_required: true,
            grant_scope: "principal_scoped_browser_engine_grant".to_string(),
            capsule_contract: "browser -> browser-engine capability -> Browser Engine Adapter"
                .to_string(),
            source: "local_provider".to_string(),
            runtime_contract: Some(runtime_contract),
            contact_id: None,
            capsule_hint: Some("browser".to_string()),
            route: Some("/apps/browser/".to_string()),
        });
    }
    if data_dir.join("bin/ipfs-provider").is_file() {
        offers.push(HomeServiceOfferSummary {
            schema: "elastos.service.offer/v1".to_string(),
            offer_id: "local:provider:content-availability".to_string(),
            service_uri: "elastos://content/availability".to_string(),
            service_kind: "content_availability".to_string(),
            display_name: "Content availability".to_string(),
            provider_uri: Some("elastos://content/*".to_string()),
            provider_label: "Content provider / IPFS backend".to_string(),
            policy_summary: "Pins and republishes selected content through provider-owned availability policy; capsules use content grants instead of raw IPFS authority.".to_string(),
            status: "configured".to_string(),
            enabled: true,
            grant_required: true,
            grant_scope: "principal_scoped_content_grant".to_string(),
            capsule_contract: "capsule -> content capability -> availability provider -> ipfs-provider".to_string(),
            source: "local_provider".to_string(),
            runtime_contract: None,
            contact_id: None,
            capsule_hint: Some(LIBRARY_CAPSULE_ID.to_string()),
            route: Some(format!("/apps/{LIBRARY_CAPSULE_ID}/")),
        });
    }
    if data_dir.join("bin/object-provider").is_file() {
        offers.push(HomeServiceOfferSummary {
            schema: "elastos.service.offer/v1".to_string(),
            offer_id: "local:provider:object-storage".to_string(),
            service_uri: "elastos://object/storage".to_string(),
            service_kind: "object_storage".to_string(),
            display_name: "Object storage".to_string(),
            provider_uri: Some("elastos://object/*".to_string()),
            provider_label: "Object provider".to_string(),
            policy_summary: "Can back spaces and shares; Object provider enforces namespace grants before capsules read or write.".to_string(),
            status: "configured".to_string(),
            enabled: true,
            grant_required: true,
            grant_scope: "principal_scoped_object_grant".to_string(),
            capsule_contract: "capsule -> object capability -> Object provider".to_string(),
            source: "local_provider".to_string(),
            runtime_contract: None,
            contact_id: None,
            capsule_hint: Some(LIBRARY_CAPSULE_ID.to_string()),
            route: Some(format!("/apps/{LIBRARY_CAPSULE_ID}/")),
        });
    }
    if data_dir.join("bin/webspace-provider").is_file() {
        offers.push(HomeServiceOfferSummary {
            schema: "elastos.service.offer/v1".to_string(),
            offer_id: "local:provider:webspace-hosting".to_string(),
            service_uri: "elastos://site/hosting".to_string(),
            service_kind: "webspace_hosting".to_string(),
            display_name: "Webspace hosting".to_string(),
            provider_uri: Some("elastos://webspace/*".to_string()),
            provider_label: "Webspace provider".to_string(),
            policy_summary: "Can publish selected site objects; Webspace provider enforces release, quota, and publication grants.".to_string(),
            status: "configured".to_string(),
            enabled: true,
            grant_required: true,
            grant_scope: "principal_scoped_webspace_grant".to_string(),
            capsule_contract: "capsule -> publish capability -> Webspace provider".to_string(),
            source: "local_provider".to_string(),
            runtime_contract: None,
            contact_id: None,
            capsule_hint: None,
            route: None,
        });
    }
    offers
}

fn home_profile_card_from_room_profile(
    card: crate::room_service::RoomProfileCardView,
) -> HomeProfileCardSummary {
    HomeProfileCardSummary {
        schema: card.schema,
        profile_id: card.profile_id,
        display_name: card.display_name,
        handle: card.handle,
        updated_at: card.updated_at,
    }
}

pub(super) fn home_people_contact_id(member_did: &str) -> String {
    let digest = Sha256::digest(format!("elastos.people.contact.v1:{member_did}").as_bytes());
    format!("contact:{}", hex::encode(&digest[..12]))
}

fn home_people_fallback_display_name(role: &crate::room_service::RoomRole, index: usize) -> String {
    match role {
        crate::room_service::RoomRole::Owner => "Conversation host".to_string(),
        crate::room_service::RoomRole::Admin => "Conversation manager".to_string(),
        crate::room_service::RoomRole::Member => format!("Conversation member {}", index + 1),
    }
}

fn home_room_role_label(role: crate::room_service::RoomRole) -> String {
    match role {
        crate::room_service::RoomRole::Owner => "owner",
        crate::room_service::RoomRole::Admin => "admin",
        crate::room_service::RoomRole::Member => "member",
    }
    .to_string()
}

fn home_notifications_summary(
    summary: crate::notifications::NotificationSummary,
) -> HomeNotificationsSummary {
    HomeNotificationsSummary {
        unread_count: summary.unread_count,
        attention_count: summary.attention_count,
        entries: summary
            .entries
            .into_iter()
            .map(|entry| HomeNotificationEntrySummary {
                id: entry.id,
                source_app: entry.source_app,
                kind: entry.kind,
                title: entry.title,
                body: entry.body,
                action_ref: entry
                    .action_ref
                    .map(|action_ref| HomeNotificationActionSummary {
                        app: action_ref.app,
                        action_id: action_ref.action_id,
                    }),
                severity: home_notification_severity(entry.severity).to_string(),
                read: entry.read,
                created_at: entry.created_at,
            })
            .collect(),
    }
}

fn home_notification_severity(
    severity: crate::notifications::NotificationSeverity,
) -> &'static str {
    match severity {
        crate::notifications::NotificationSeverity::Info => "info",
        crate::notifications::NotificationSeverity::Attention => "attention",
        crate::notifications::NotificationSeverity::Critical => "critical",
    }
}

#[derive(Deserialize)]
struct HomeAttachResponse {
    token: String,
}

#[derive(Deserialize)]
struct HomeCapsulesResponse {
    capsules: Vec<HomeCapsuleInfo>,
}

#[derive(Deserialize)]
struct HomeCapsuleInfo {
    name: String,
}

pub(super) async fn home_runtime_summary(data_dir: &std::path::Path) -> HomeRuntimeSummary {
    let Some(coords) = load_live_runtime_coords(data_dir).await else {
        return HomeRuntimeSummary {
            running: false,
            note: Some("No active local runtime".to_string()),
            ..HomeRuntimeSummary::default()
        };
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return HomeRuntimeSummary {
                running: true,
                kind: Some(coords.runtime_kind.clone()),
                api_url: Some(coords.api_url.clone()),
                pid: Some(coords.pid),
                note: Some(format!("Runtime client unavailable: {err}")),
                ..HomeRuntimeSummary::default()
            };
        }
    };

    let mut runtime = HomeRuntimeSummary {
        running: true,
        kind: Some(coords.runtime_kind.clone()),
        version: Some(GATEWAY_VERSION.to_string()),
        api_url: Some(coords.api_url.clone()),
        pid: Some(coords.pid),
        running_capsules: Vec::new(),
        note: None,
    };

    let home_token = match home_attach_shell(&client, &coords.api_url, &coords.attach_secret).await
    {
        Ok(token) => token,
        Err(err) => {
            runtime.note = Some(format!("Runtime attach failed: {err}"));
            return runtime;
        }
    };

    match home_list_runtime_capsules(&client, &coords.api_url, &home_token).await {
        Ok(capsules) => runtime.running_capsules = capsules,
        Err(err) => {
            runtime.note = Some(format!(
                "Runtime attached, but capsule list is unavailable: {err}"
            ))
        }
    }

    runtime
}

pub(super) async fn ensure_home_runtime(data_dir: &std::path::Path) -> HomeRuntimeSummary {
    match crate::runtime_control::ensure_runtime_for_home(data_dir).await {
        Ok(_) => home_runtime_summary(data_dir).await,
        Err(err) => HomeRuntimeSummary {
            running: false,
            note: Some(format!("Managed local runtime could not start: {err}")),
            ..HomeRuntimeSummary::default()
        },
    }
}

pub(super) async fn load_live_runtime_coords(
    data_dir: &std::path::Path,
) -> Option<crate::runtime_control::RuntimeCoords> {
    let path = crate::runtime_control::runtime_coord_path(data_dir);
    crate::runtime_control::read_runtime_coords(&path).await
}

pub(super) async fn home_attach_shell(
    client: &reqwest::Client,
    api_url: &str,
    attach_secret: &str,
) -> anyhow::Result<String> {
    gateway_attach_runtime_token(client, api_url, attach_secret, "shell").await
}

pub(super) async fn gateway_attach_runtime_token(
    client: &reqwest::Client,
    api_url: &str,
    attach_secret: &str,
    scope: &str,
) -> anyhow::Result<String> {
    Ok(client
        .post(format!("{}/api/auth/attach", api_url))
        .json(&serde_json::json!({
            "secret": attach_secret,
            "scope": scope,
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<HomeAttachResponse>()
        .await?
        .token)
}

async fn home_list_runtime_capsules(
    client: &reqwest::Client,
    api_url: &str,
    home_token: &str,
) -> anyhow::Result<Vec<String>> {
    let response = client
        .get(format!("{}/api/capsules", api_url))
        .header(AUTHORIZATION, format!("Bearer {home_token}"))
        .send()
        .await?
        .error_for_status()?
        .json::<HomeCapsulesResponse>()
        .await?;
    let mut names = response
        .capsules
        .into_iter()
        .map(|capsule| capsule.name)
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_home_launch_token_drops_reserved_query_token() {
        let dir = tempfile::tempdir().unwrap();
        let context = local_home_launch_token_context(dir.path()).unwrap();
        let mut query = BTreeMap::new();
        query.insert("home_token".to_string(), "attacker".to_string());
        query.insert("capsule".to_string(), "documents".to_string());

        let delivery = append_home_launch_token(
            dir.path(),
            "/apps/documents/",
            "documents",
            &query,
            &context,
            false,
        )
        .unwrap();
        assert!(
            delivery.set_cookie.is_none(),
            "ordinary apps keep URL delivery"
        );
        let route = delivery.route;
        let parsed = url::Url::parse(&format!("http://localhost{route}")).unwrap();
        let query_pairs = parsed.query_pairs().collect::<Vec<_>>();
        let home_tokens = query_pairs
            .iter()
            .filter(|(key, _)| key == "home_token")
            .collect::<Vec<_>>();

        assert_eq!(home_tokens.len(), 1);
        assert_ne!(home_tokens[0].1.as_ref(), "attacker");
        assert!(query_pairs
            .iter()
            .any(|(key, value)| key == "capsule" && value == "documents"));
    }

    /// Sprint 33 ratchet (council S31 F1): the MANDATES launch URL carries NO bearer credential —
    /// the token rides an HttpOnly, SameSite=Strict, path-scoped Set-Cookie instead, and the URL
    /// keeps only the non-secret `shell=1` in-shell marker. Before this sprint the exact failure
    /// was `/apps/mandates/?home_token=<12h bearer token>` — copyable from history, logs, and
    /// frame script.
    #[test]
    fn mandates_launch_url_carries_no_token_and_the_cookie_carries_it_instead() {
        let dir = tempfile::tempdir().unwrap();
        let context = local_home_launch_token_context(dir.path()).unwrap();
        let mut query = BTreeMap::new();
        // A smuggled marker/token in the incoming query must not survive either.
        query.insert("home_token".to_string(), "attacker".to_string());
        query.insert("shell".to_string(), "attacker".to_string());

        let delivery = append_home_launch_token(
            dir.path(),
            "/apps/mandates/",
            super::super::gateway_mandates::MANDATES_CAPSULE_ID,
            &query,
            &context,
            true,
        )
        .unwrap();

        assert!(
            !delivery.route.contains("home_token"),
            "the money surface's launch URL must never carry the bearer token: {}",
            delivery.route
        );
        let parsed = url::Url::parse(&format!("http://localhost{}", delivery.route)).unwrap();
        let shell_markers: Vec<_> = parsed
            .query_pairs()
            .filter(|(key, _)| key == "shell")
            .collect();
        assert_eq!(shell_markers.len(), 1, "exactly one in-shell marker");
        assert_eq!(shell_markers[0].1.as_ref(), "1", "the smuggled marker is dropped");

        let cookie = delivery
            .set_cookie
            .expect("the launch response must deliver the token via Set-Cookie");
        let cookie = cookie.to_str().unwrap();
        assert!(cookie.starts_with(&format!("{MANDATES_SESSION_COOKIE}=")));
        assert!(cookie.contains("HttpOnly"), "frame script must not read it: {cookie}");
        assert!(cookie.contains("SameSite=Strict"), "never sent cross-site: {cookie}");
        assert!(
            cookie.contains(&format!("Path={MANDATES_API_COOKIE_PATH}")),
            "scoped to the mandates API alone: {cookie}"
        );
        assert!(cookie.contains("Secure"), "TLS launches set Secure: {cookie}");
        // The cookie value IS a valid launch token for the mandates app.
        let token = cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{MANDATES_SESSION_COOKIE}="))
            .to_string();
        let mut headers = HeaderMap::new();
        headers.insert("x-elastos-home-token", token.parse().unwrap());
        super::super::require_home_launch_token(
            dir.path(),
            &headers,
            super::super::gateway_mandates::MANDATES_CAPSULE_ID,
        )
        .expect("the cookie-delivered token authorizes the mandates surface");
    }
}
