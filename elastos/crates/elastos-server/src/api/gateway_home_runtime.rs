use super::*;

pub(super) async fn home_launch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<HomeLaunchRequest>,
) -> Result<Json<HomeLaunchResponse>, (StatusCode, Json<serde_json::Value>)> {
    let context = require_home_launch_token_context(&state.data_dir, &headers, HOME_CAPSULE_ID)
        .map_err(|err| {
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

    // Launch is the authenticated mutation boundary for state that used to
    // be changed by the target's first summary GET. Existing-only Profile
    // registration starts Runtime-owned contact sync without creating
    // identity, Profile, device, or contact state. Inbox launch retains the
    // legacy Services mailbox behavior while keeping Inbox summary pure.
    let data_dir = state.data_dir.clone();
    let launch_context = context.clone();
    let sync_services = target_summary.target == INBOX_CAPSULE_ID;
    let services_sync_error = tokio::task::spawn_blocking(move || {
        super::gateway_home_system::migrate_legacy_services_peer_contacts(
            &data_dir,
            &launch_context,
        )?;
        Ok::<_, anyhow::Error>(sync_services.then(|| {
            super::gateway_home_system::home_services_sync_access_requests(
                &data_dir,
                &launch_context,
            )
            .err()
            .map(|error| error.to_string())
        }))
    })
    .await
    .map_err(|err| gateway_internal_error(anyhow::anyhow!(err)))?
    .map_err(gateway_internal_error)?
    .flatten();
    if let Some(error) = services_sync_error {
        tracing::warn!(
            error,
            "Inbox launch could not sync Services access requests"
        );
    }
    if let Some(service) = state.collaboration_discovery_service.as_ref() {
        if let Some(authority) =
            super::gateway_home_system::load_configured_contact_authority_for_context(
                &state.data_dir,
                &context,
                Some(service),
            )
            .map_err(gateway_internal_error)?
        {
            super::gateway_home_system::register_configured_contact_sync_for_context(
                service,
                &context,
                &authority,
                now_ts(),
            )
            .map_err(gateway_internal_error)?;
        }
    }

    let launch = launch_runtime_backed_home_target(
        &state.data_dir,
        target_summary.target.as_str(),
        &context,
    )
    .await;
    let executable_actor = target_summary
        .viewer
        .as_deref()
        .unwrap_or(target_summary.target.as_str());
    let route = append_home_launch_token(
        &state.data_dir,
        &target_summary.route,
        &target_summary.target,
        executable_actor,
        &req.query,
        &context,
    )
    .map_err(gateway_internal_error)?;
    let route =
        crate::api::browser_capsules::canonical_browser_capsule_route(&route).map_err(|error| {
            (
                StatusCode::MISDIRECTED_REQUEST,
                Json(serde_json::json!({ "error": error })),
            )
        })?;

    Ok(Json(HomeLaunchResponse {
        target: target_summary.target,
        title: target_summary.title,
        route,
        attach_kind: target_summary.attach_kind,
        role: target_summary.role,
        target_kind: target_summary.target_kind,
        viewer: target_summary.viewer,
        viewer_title: target_summary.viewer_title,
        launch_status: launch.as_ref().map(|summary| summary.status.clone()),
        launch_detail: launch.as_ref().and_then(|summary| summary.detail.clone()),
        capsule_id: launch.and_then(|summary| summary.capsule_id),
    }))
}

pub(super) fn append_home_launch_token(
    data_dir: &std::path::Path,
    route: &str,
    selected_resource: &str,
    executable_actor: &str,
    query: &BTreeMap<String, String>,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<String> {
    let token = issue_home_projection_launch_token_with_context(
        data_dir,
        selected_resource,
        executable_actor,
        context,
    )?;
    append_home_launch_token_to_route(route, query, &token)
}

fn append_home_launch_token_to_route(
    route: &str,
    query: &BTreeMap<String, String>,
    token: &str,
) -> anyhow::Result<String> {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in query {
        let key = key.trim();
        if key.is_empty() || key == "home_token" {
            continue;
        }
        serializer.append_pair(key, value);
    }
    let encoded = serializer.finish();
    let route = if encoded.is_empty() {
        route.to_string()
    } else {
        let separator = if route.contains('?') { '&' } else { '?' };
        format!("{route}{separator}{encoded}")
    };
    let mut fragment = form_urlencoded::Serializer::new(String::new());
    fragment.append_pair("home_token", token);
    Ok(format!("{route}#{}", fragment.finish()))
}

/// Sizes every capsule that declares an icon must ship.
const CAPSULE_ICON_SIZES: [u32; 4] = [32, 64, 128, 256];

/// Turns a manifest's declared icon directory into capsule asset routes.
///
/// The Runtime resolves the routes; it never reads or re-hosts the bytes. A
/// capsule that declares no icon gets an empty list, and the shell draws its
/// own generic glyph instead of guessing at a path.
///
/// The asset route serves paths relative to the entrypoint's directory (for a
/// `browser/index.html` capsule the serving root is `browser/`), while the
/// manifest declares the icon capsule-relative like the entrypoint itself. So
/// the declared directory must sit under the entrypoint's directory, and the
/// route carries the remainder. An icon outside the serving root has no
/// servable route, and pretending otherwise would hand the shell a 404.
pub(super) fn capsule_icon_variants(
    name: &str,
    entrypoint: &str,
    icon: Option<&str>,
) -> Vec<CapsuleIconVariant> {
    let Some(dir) = icon.map(str::trim).filter(|dir| !dir.is_empty()) else {
        return Vec::new();
    };
    let dir = dir.trim_end_matches('/');
    let entry_dir = match entrypoint.rsplit_once('/') {
        Some((parent, _file)) => parent,
        None => "",
    };
    let served = if entry_dir.is_empty() {
        dir
    } else if let Some(rest) = dir
        .strip_prefix(entry_dir)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        rest
    } else {
        return Vec::new();
    };
    CAPSULE_ICON_SIZES
        .iter()
        .map(|size| CapsuleIconVariant {
            size: *size,
            route: format!("/apps/{name}/{served}/icon-{size}.png"),
        })
        .collect()
}

pub(super) fn home_targets(data_dir: &std::path::Path) -> Vec<HomeTargetSummary> {
    let catalog = capsule_catalog_summary(data_dir);
    home_targets_from_catalog(&catalog)
}

pub(super) fn home_targets_from_catalog(
    catalog: &CapsuleCatalogResponse,
) -> Vec<HomeTargetSummary> {
    let mut targets = catalog
        .capsules
        .iter()
        .filter(|capsule| capsule.launchable)
        .filter(|capsule| capsule.role != CapsuleRole::Shell)
        .filter(|capsule| is_home_visible_target(&capsule.name))
        .filter_map(|capsule| {
            Some(HomeTargetSummary {
                target: capsule.launch_target.clone()?,
                title: capsule.title.clone(),
                description: capsule.description.clone(),
                route: capsule.route.clone()?,
                attach_kind: "iframe".to_string(),
                role: capsule.role.clone(),
                target_kind: if capsule.role == CapsuleRole::Content {
                    HomeTargetKind::Object
                } else {
                    HomeTargetKind::App
                },
                viewer: capsule.viewer.clone(),
                viewer_title: capsule.viewer_title.clone(),
                icon: capsule.icon.clone(),
            })
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| {
        left.title
            .cmp(&right.title)
            .then_with(|| left.target.cmp(&right.target))
    });
    targets
}

pub(super) fn home_launch_targets(data_dir: &std::path::Path) -> Vec<HomeTargetSummary> {
    let mut targets = home_browser_targets(data_dir, false);
    targets.extend(home_viewer_targets(data_dir));
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

pub(super) fn home_launch_target(
    data_dir: &std::path::Path,
    target: &str,
) -> Option<HomeTargetSummary> {
    home_launch_targets(data_dir)
        .into_iter()
        .find(|candidate| candidate.target == target)
}

fn home_browser_targets(data_dir: &std::path::Path, visible_only: bool) -> Vec<HomeTargetSummary> {
    let mut targets: Vec<_> =
        crate::api::browser_capsules::list_launchable_browser_capsules(data_dir)
            .into_iter()
            .filter(|app| app.name != HOME_CAPSULE_ID)
            .filter(|app| {
                !visible_only
                    || (app.role != CapsuleRole::Shell && is_home_visible_target(&app.name))
            })
            .map(|app| HomeTargetSummary {
                route: format!("/apps/{}/", app.name),
                title: app_shell_title(&app.name),
                description: app_shell_description(&app.name, app.description),
                icon: capsule_icon_variants(&app.name, &app.entrypoint, app.icon.as_deref()),
                target: app.name,
                attach_kind: "iframe".to_string(),
                role: app.role,
                target_kind: HomeTargetKind::App,
                viewer: None,
                viewer_title: None,
            })
            .collect();
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

fn home_viewer_targets(data_dir: &std::path::Path) -> Vec<HomeTargetSummary> {
    let mut targets = crate::api::browser_capsules::list_all_viewer_bound_capsules(data_dir)
        .into_iter()
        .map(|capsule| {
            let viewer_title = app_shell_title(&capsule.viewer);
            let icon =
                capsule_icon_variants(&capsule.name, &capsule.entrypoint, capsule.icon.as_deref());
            HomeTargetSummary {
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
                // A content capsule may ship its own icon; when it does not,
                // the shell draws the object glyph rather than borrowing the
                // viewer app's identity for a document.
                icon,
                viewer: Some(capsule.viewer),
                viewer_title: Some(viewer_title),
            }
        })
        .collect::<Vec<_>>();
    targets.sort_by(|left, right| left.title.cmp(&right.title));
    targets
}

pub(super) fn is_home_visible_target(name: &str) -> bool {
    !matches!(
        name,
        WALLET_METAMASK_CAPSULE_ID | WALLET_UNISAT_CAPSULE_ID | WALLET_WALLETCONNECT_CAPSULE_ID
    )
}

fn load_gateway_device_did(data_dir: &std::path::Path) -> anyhow::Result<Option<String>> {
    crate::collaboration_profile_authority::load_existing_device_did(data_dir)
}

impl HomeIdentitySummary {
    /// The capsule-facing shape: identity facts minus the device identity.
    /// System alone renders the device DID; Home and People have no consumer
    /// for it, so they never receive it.
    pub(super) fn without_device_identity(mut self) -> Self {
        self.device_did = None;
        self
    }
}

pub(super) fn load_gateway_identity_summary_for_context(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeIdentitySummary> {
    let (profile_readiness, profile) =
        super::gateway_home_system::home_profile_readiness_projection_for_context(
            data_dir, context,
        );
    Ok(HomeIdentitySummary {
        device_did: load_gateway_device_did(data_dir)?,
        profile_readiness: Some(profile_readiness.clone()),
        profile_setup_display_name: if profile_readiness.status == "setup_required" {
            principal_display_name_for_context(data_dir, context)
        } else {
            None
        },
        profile,
    })
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
    if manifest.is_runtime_projection() {
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
    _data_dir: &FsPath,
    capsule_dir: &FsPath,
    manifest: &elastos_common::CapsuleManifest,
) -> anyhow::Result<PathBuf> {
    let entrypoint = capsule_dir.join(&manifest.entrypoint);
    if entrypoint.is_file() || manifest.capsule_type != CapsuleType::Wasm {
        return Ok(capsule_dir.to_path_buf());
    }

    if manifest.is_component_capsule() {
        anyhow::bail!(
            "Component capsule Runtime entrypoint missing: {}",
            entrypoint.display()
        );
    }
    anyhow::bail!(
        "WASI Preview 1 product capsules are no longer materialized from target/wasm32-wasip1: {}",
        entrypoint.display()
    );
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
            "launch_grant": issue_home_projection_launch_token_with_context(
                data_dir,
                capsule_name,
                capsule_name,
                context,
            )?,
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
        | AuditEvent::AuthAttempt { timestamp, .. }
        | AuditEvent::EpochAdvance { timestamp, .. }
        | AuditEvent::ConfigChange { timestamp, .. }
        | AuditEvent::SecurityWarning { timestamp, .. }
        | AuditEvent::SessionCreated { timestamp, .. }
        | AuditEvent::SessionDestroyed { timestamp, .. }
        | AuditEvent::CapabilityRequested { timestamp, .. }
        | AuditEvent::CapabilityDenied { timestamp, .. }
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
    crate::api::capsule_inventory::installed_active_capsule_dir(data_dir, app)
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
        HOME_CAPSULE_ID => "Home".to_string(),
        HOME_GUI_SHELL_ID => "Home GUI".to_string(),
        DOCUMENTS_CAPSULE_ID => "Documents".to_string(),
        CHAT_ROOM_CAPSULE_ID => "Chat".to_string(),
        LIBRARY_CAPSULE_ID => "Library".to_string(),
        MARKETPLACE_CAPSULE_ID => "Marketplace".to_string(),
        PEOPLE_CAPSULE_ID => "People".to_string(),
        INBOX_CAPSULE_ID => "Inbox".to_string(),
        SERVICES_CAPSULE_ID => "Services".to_string(),
        SYSTEM_CAPSULE_ID => "System".to_string(),
        BROWSER_CAPSULE_ID => "Browser".to_string(),
        WALLET_CAPSULE_ID => "Wallet".to_string(),
        "archive-manager" => "Archive".to_string(),
        "home-cli" => "Home CLI".to_string(),
        "gba-emulator" => "GBA Emulator".to_string(),
        _ if is_wallet_connector_capsule_id(name) => wallet_connector_label(name).to_string(),
        _ => title_case_capsule_name(name),
    }
}

fn app_shell_description(name: &str, manifest_description: Option<String>) -> String {
    match name {
        DOCUMENTS_CAPSULE_ID => {
            "Create, edit, and publish markdown documents from this device.".to_string()
        }
        CHAT_ROOM_CAPSULE_ID => "Send messages and join conversations.".to_string(),
        LIBRARY_CAPSULE_ID => "Browse documents and open them in Documents.".to_string(),
        MARKETPLACE_CAPSULE_ID => {
            "Browse installed apps, services, viewers, and content.".to_string()
        }
        PEOPLE_CAPSULE_ID => "Manage people and local discovery.".to_string(),
        INBOX_CAPSULE_ID => "Review messages, requests, and approvals.".to_string(),
        SERVICES_CAPSULE_ID => "Manage Browser Exit Node sharing and subscriptions.".to_string(),
        SYSTEM_CAPSULE_ID => "Manage passkeys, appearance, and Home settings.".to_string(),
        BROWSER_CAPSULE_ID => "Browse websites from this device.".to_string(),
        WALLET_CAPSULE_ID => {
            "View accounts, balances, approvals, and approval methods.".to_string()
        }
        "archive-manager" => "Open archives selected from Library.".to_string(),
        _ if is_wallet_connector_capsule_id(name) => format!(
            "Add {} as an approval method.",
            wallet_connector_label(name)
        ),
        "gba-emulator" => "Play GBA games from Library.".to_string(),
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
        .map(|member| {
            let active = active_by_member.get(&member.member_did);
            let profile_card = member
                .profile_card
                .clone()
                .map(home_profile_card_from_room_profile);
            // A conversation member is named by its signed profile card or is
            // explicitly unverified. Session self-claims and invented role
            // names are not identity sources.
            let display_name = profile_card
                .as_ref()
                .map(|card| card.display_name.clone())
                .unwrap_or_else(|| HOME_PEOPLE_UNVERIFIED_MEMBER_NAME.to_string());
            HomePeopleContactSummary {
                contact_id: home_people_contact_id(&member.member_did),
                remote_profile_did: profile_card.as_ref().map(|card| card.profile_id.clone()),
                added_at: member.added_at,
                conversation_id: None,
                display_name,
                handle: profile_card.as_ref().and_then(|card| card.handle.clone()),
                relationship: "conversation".to_string(),
                can_message: true,
                device_label: active.map(|participant| participant.device_label.clone()),
                profile_card,
                last_seen_at: active.map(|participant| participant.last_seen_at),
                // Room membership rows have no presence basis of their own; a
                // room participant with an active session is the signal here.
                reachable: None,
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

/// The one honest name for a member row with no signed profile card. Chat
/// renders the same marker for `profile_verified: false` rows.
pub(super) const HOME_PEOPLE_UNVERIFIED_MEMBER_NAME: &str = "Unverified device";

pub(super) fn home_people_contact_id(member_did: &str) -> String {
    let digest = Sha256::digest(format!("elastos.people.contact.v1:{member_did}").as_bytes());
    format!("contact:{}", hex::encode(&digest[..12]))
}

fn home_room_role_label(role: crate::room_service::RoomRole) -> String {
    match role {
        crate::room_service::RoomRole::Owner => "owner",
        crate::room_service::RoomRole::Admin => "admin",
        crate::room_service::RoomRole::Member => "member",
    }
    .to_string()
}

pub(super) fn home_notifications_summary(
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
    let path = crate::runtime_control::home_runtime_coord_path(data_dir);
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
    fn declared_capsule_icon_resolves_to_that_capsule_own_asset_routes() {
        // The asset route serves relative to the entrypoint's directory, so a
        // browser/index.html capsule serves browser/icons at /icons.
        let variants = capsule_icon_variants("people", "browser/index.html", Some("browser/icons"));

        let routes = variants
            .iter()
            .map(|variant| (variant.size, variant.route.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            routes,
            vec![
                (32, "/apps/people/icons/icon-32.png"),
                (64, "/apps/people/icons/icon-64.png"),
                (128, "/apps/people/icons/icon-128.png"),
                (256, "/apps/people/icons/icon-256.png"),
            ],
            "each route must point at the declaring capsule, never at a shell icon table"
        );

        // A flat capsule (entrypoint at the root) serves its icon dir as-is.
        let flat = capsule_icon_variants("viewer", "index.html", Some("icons"));
        assert_eq!(flat[0].route, "/apps/viewer/icons/icon-32.png");
    }

    #[test]
    fn a_capsule_that_declares_no_icon_gets_no_route_to_guess_at() {
        assert!(capsule_icon_variants("people", "browser/index.html", None).is_empty());
        assert!(capsule_icon_variants("people", "browser/index.html", Some("   ")).is_empty());
    }

    #[test]
    fn an_icon_outside_the_serving_root_gets_no_route_rather_than_a_broken_one() {
        // Declared beside — not under — the entrypoint's directory: the asset
        // route cannot serve it, so the shell must get nothing, not a 404.
        assert!(
            capsule_icon_variants("people", "browser/index.html", Some("assets/icons")).is_empty()
        );
        assert!(
            capsule_icon_variants("people", "browser/index.html", Some("browserx/icons"))
                .is_empty()
        );
    }

    #[test]
    fn a_declared_icon_directory_may_not_escape_the_capsule() {
        // The manifest rejects traversal before the runtime ever resolves a
        // route, so the shell can trust every route it is handed.
        let mut manifest = sample_icon_manifest();
        manifest.icon = Some("../home-gui/browser/icons".to_string());
        let error = manifest.validate().expect_err("traversal must be rejected");
        assert!(
            error.contains("path traversal"),
            "unexpected error: {error}"
        );

        manifest.icon = Some("/etc/icons".to_string());
        let error = manifest
            .validate()
            .expect_err("absolute paths must be rejected");
        assert!(error.contains("relative path"), "unexpected error: {error}");

        manifest.icon = Some("browser/icons".to_string());
        manifest
            .validate()
            .expect("a capsule-relative icon is valid");
    }

    fn sample_icon_manifest() -> elastos_common::CapsuleManifest {
        serde_json::from_value(serde_json::json!({
            "schema": "elastos.capsule/v1",
            "name": "people",
            "version": "0.1.0",
            "role": "app",
            "type": "wasm",
            "entrypoint": "browser/index.html",
        }))
        .expect("sample manifest parses")
    }

    #[test]
    fn append_home_launch_token_keeps_authority_out_of_the_request_url() {
        let dir = tempfile::tempdir().unwrap();
        let context = local_home_launch_token_context(dir.path()).unwrap();
        let mut query = BTreeMap::new();
        query.insert("home_token".to_string(), "attacker".to_string());
        query.insert("capsule".to_string(), "documents".to_string());

        let route = append_home_launch_token(
            dir.path(),
            "/apps/documents/",
            "documents",
            "documents",
            &query,
            &context,
        )
        .unwrap();
        let parsed = url::Url::parse(&format!("http://localhost{route}")).unwrap();
        let query_pairs = parsed.query_pairs().collect::<Vec<_>>();
        let fragment_pairs =
            form_urlencoded::parse(parsed.fragment().unwrap_or_default().as_bytes())
                .collect::<Vec<_>>();

        assert!(query_pairs.iter().all(|(key, _)| key != "home_token"));
        assert_eq!(
            fragment_pairs
                .iter()
                .filter(|(key, _)| key == "home_token")
                .count(),
            1
        );
        assert!(fragment_pairs
            .iter()
            .any(|(key, value)| key == "home_token" && value != "attacker"));
        assert!(query_pairs
            .iter()
            .any(|(key, value)| key == "capsule" && value == "documents"));
    }
}
