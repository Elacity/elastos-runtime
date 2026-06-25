use super::*;

pub(super) async fn home_launch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<HomeLaunchRequest>,
) -> Result<Json<HomeLaunchResponse>, (StatusCode, Json<serde_json::Value>)> {
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
    let route = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        target_summary.target.as_str(),
        &req.query,
        &context,
    )
    .map_err(gateway_internal_error)?;

    Ok(Json(HomeLaunchResponse {
        target: target_summary.target,
        title: target_summary.title,
        route,
        attach_kind: target_summary.attach_kind,
        role: target_summary.role,
        target_kind: target_summary.target_kind,
        launch_status: launch.as_ref().map(|summary| summary.status.clone()),
        launch_detail: launch.as_ref().and_then(|summary| summary.detail.clone()),
        capsule_id: launch.and_then(|summary| summary.capsule_id),
    }))
}

pub(super) fn append_home_launch_token(
    data_dir: &std::path::Path,
    route: &str,
    target: &str,
    query: &BTreeMap<String, String>,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<String> {
    let token = issue_home_launch_token_with_context(data_dir, target, context)?;
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("home_token", &token);
    for (key, value) in query {
        if key.trim().is_empty() {
            continue;
        }
        serializer.append_pair(key, value);
    }
    let encoded = serializer.finish();
    let separator = if route.contains('?') { '&' } else { '?' };
    Ok(format!("{route}{separator}{encoded}"))
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
    HomeIdentitySummary {
        device_did: load_gateway_device_did(data_dir),
        handle: principal_display_name_for_context(data_dir, context),
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
        return Ok(capsule_dir.to_path_buf());
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
    let installed_dir = data_dir.join("capsules").join(target);
    if !required && !installed_dir.exists() {
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
        CHAT_ROOM_CAPSULE_ID => "Chat Room".to_string(),
        LIBRARY_CAPSULE_ID => "Library".to_string(),
        MARKETPLACE_CAPSULE_ID => "Marketplace".to_string(),
        INBOX_CAPSULE_ID => "Inbox".to_string(),
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
        CHAT_ROOM_CAPSULE_ID => {
            "Open the local sovereign room from this runtime inside ElastOS.".to_string()
        }
        LIBRARY_CAPSULE_ID => "Browse documents and open them in Documents.".to_string(),
        MARKETPLACE_CAPSULE_ID => {
            "Browse installed capsules, providers, viewers, and content.".to_string()
        }
        INBOX_CAPSULE_ID => "Review requests and approvals for this Home.".to_string(),
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
    let _ = crate::notifications::sync_room_notifications(data_dir, &room_summary);
    let notifications = crate::notifications::load_summary(data_dir).unwrap_or_default();

    HomeState {
        site,
        room: home_room_summary(room_summary),
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
