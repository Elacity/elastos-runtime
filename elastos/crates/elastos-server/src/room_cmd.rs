use serde::Serialize;
use std::path::Path;

use elastos_server::browser_app_hosts::load_browser_app_hosted_endpoint;
use elastos_server::notifications::sync_room_notifications;
use elastos_server::room_service::{
    approve_next_request, approve_request, deny_next_request, deny_request, load_summary,
    local_runtime_access, reset_room, room_root_uri, room_slug, ApprovalOutcome, DenyOutcome,
    PendingRequestView, RoomResetOutput, RoomRole, RoomSummary,
};
use elastos_server::sources::default_data_dir;

#[derive(Debug, Serialize)]
struct RoomShowOutput {
    room_root_uri: &'static str,
    #[serde(flatten)]
    summary: RoomSummary,
}

#[derive(Debug, Serialize)]
struct RoomPendingOutput {
    room_slug: &'static str,
    pending_requests: Vec<PendingRequestView>,
}

pub async fn run_room(cmd: crate::RoomCommand) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    match cmd {
        crate::RoomCommand::Show { json } => run_show(&data_dir, json).await,
        crate::RoomCommand::Pending { json } => run_pending(&data_dir, json).await,
        crate::RoomCommand::Approve { request_id, json } => {
            run_approve(&data_dir, request_id, json).await
        }
        crate::RoomCommand::Deny {
            request_id,
            reason,
            json,
        } => run_deny(&data_dir, request_id, reason, json).await,
        crate::RoomCommand::Open { addr } => run_open(&data_dir, addr).await,
        crate::RoomCommand::Reset { json } => run_reset(&data_dir, json).await,
    }
}

async fn run_show(data_dir: &Path, json: bool) -> anyhow::Result<()> {
    let summary = load_room_summary_with_local_context(data_dir).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&RoomShowOutput {
                room_root_uri: room_root_uri(),
                summary,
            })?
        );
        return Ok(());
    }

    println!("Chat:       {}", summary.room_control.title);
    println!("Slug:       {}", summary.room_slug);
    println!("Root:       {}", room_root_uri());
    if let Some(role) = summary.local_runtime_role.as_ref() {
        println!("Role:       {}", role_label(role));
    } else {
        println!("Access:     this device is not connected to this conversation");
    }
    println!(
        "ElastOS:    {}",
        summary
            .local_runtime_did
            .as_deref()
            .unwrap_or("(local DID unavailable)")
    );
    println!(
        "Guests:     {}",
        if summary.room_control.access_policy.allow_guest_invites {
            "public join requests enabled"
        } else {
            "public join requests disabled"
        }
    );
    println!(
        "ElastOS:    {}",
        if summary.room_control.access_policy.allow_member_invites {
            "user invites enabled"
        } else {
            "user invites disabled"
        }
    );
    println!(
        "Approvals:  {}",
        if summary
            .room_control
            .access_policy
            .allow_members_to_host_guests
        {
            "trusted users may approve web guests"
        } else {
            "conversation managers approve web guests"
        }
    );
    if let Some(url) = summary.canonical_hosted_guest_url.as_deref() {
        println!("Public URL: {}", url);
    }
    if let Some(url) = summary.ephemeral_hosted_guest_url.as_deref() {
        println!("Quick URL:  {}", url);
    }
    println!(
        "People:     {} trusted, {} active, {} manager",
        summary.room_control.member_count,
        summary.room_control.active_member_count,
        summary.room_control.admin_count
    );
    if summary.room_control.owner_did.is_none() {
        println!("Owner:      not seeded yet");
    }
    if summary.pending_requests.is_empty() {
        println!("Pairing:    no pending browser requests");
    } else {
        println!(
            "Pairing:    {} pending browser request(s)",
            summary.pending_requests.len()
        );
        for request in summary.pending_requests.iter().take(5) {
            println!(
                "  - {} on {} (id: {})",
                request.display_name, request.device_label, request.request_id
            );
        }
    }
    if summary.room_control.pending_invites.is_empty() {
        println!("Invites:    none pending");
    } else {
        println!(
            "Invites:    {} pending",
            summary.room_control.pending_invites.len()
        );
        for invite in summary.room_control.pending_invites.iter().take(5) {
            println!(
                "  - {} as {} (id: {})",
                invite.invited_did,
                role_label(&invite.role),
                invite.invite_id
            );
        }
    }
    if summary.room_control.owner_did.is_none() {
        println!("Hint:       Use authenticated People or Chat to create room membership.");
    } else {
        println!("Hint:       Use authenticated People or Chat to manage room membership.");
        println!("Next:       elastos room pending");
        println!("Open:       elastos room open --addr 0.0.0.0:8090");
    }
    Ok(())
}

async fn run_pending(data_dir: &Path, json: bool) -> anyhow::Result<()> {
    let summary = load_room_summary_with_local_context(data_dir).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&RoomPendingOutput {
                room_slug: room_slug(),
                pending_requests: summary.pending_requests,
            })?
        );
        return Ok(());
    }

    if summary.pending_requests.is_empty() {
        println!("No pending room browser requests.");
        return Ok(());
    }

    println!(
        "Pending room browser requests: {}",
        summary.pending_requests.len()
    );
    for request in &summary.pending_requests {
        println!(
            "- {} on {} (id: {}, requested: {})",
            request.display_name, request.device_label, request.request_id, request.requested_at
        );
    }
    println!("Approve: elastos room approve <request-id>");
    println!("Deny:    elastos room deny <request-id> --reason \"Denied from CLI\"");
    Ok(())
}

async fn run_approve(
    data_dir: &Path,
    request_id: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let outcome = match request_id.as_deref() {
        Some(request_id) => approve_request(data_dir, request_id)?,
        None => approve_next_request(data_dir)?,
    };

    let summary = load_room_summary_with_local_context(data_dir).await?;
    sync_room_notifications(data_dir, &summary)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        return Ok(());
    }

    match outcome {
        Some(ApprovalOutcome {
            request_id,
            display_name,
            device_label,
            expires_at,
        }) => {
            println!("Approved Chat web guest request.");
            println!("Request:    {}", request_id);
            println!("Web guest:  {} on {}", display_name, device_label);
            println!("Expires:    {}", expires_at);
        }
        None => {
            if let Some(request_id) = request_id {
                println!(
                    "That Chat web guest request is no longer pending: {}",
                    request_id
                );
            } else {
                println!("No pending Chat web guest requests.");
            }
        }
    }
    Ok(())
}

async fn run_deny(
    data_dir: &Path,
    request_id: Option<String>,
    reason: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let reason = reason.unwrap_or_else(|| "Denied from CLI".to_string());
    let outcome = match request_id.as_deref() {
        Some(request_id) => deny_request(data_dir, request_id, &reason)?,
        None => deny_next_request(data_dir, &reason)?,
    };

    let summary = load_room_summary_with_local_context(data_dir).await?;
    sync_room_notifications(data_dir, &summary)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        return Ok(());
    }

    match outcome {
        Some(DenyOutcome {
            request_id,
            display_name,
            device_label,
            reason,
        }) => {
            println!("Denied Chat web guest request.");
            println!("Request:    {}", request_id);
            println!("Web guest:  {} on {}", display_name, device_label);
            println!("Reason:     {}", reason);
        }
        None => {
            if let Some(request_id) = request_id {
                println!(
                    "That Chat web guest request is no longer pending: {}",
                    request_id
                );
            } else {
                println!("No pending Chat web guest requests.");
            }
        }
    }
    Ok(())
}

async fn run_open(data_dir: &Path, addr: String) -> anyhow::Result<()> {
    let opened =
        elastos_server::operator_control::start_local_room_gateway(data_dir, &addr).await?;
    print_local_room_open(&opened);
    Ok(())
}

async fn run_reset(data_dir: &Path, json: bool) -> anyhow::Result<()> {
    let reset = reset_room(data_dir)?;
    let summary = load_room_summary_with_local_context(data_dir).await?;
    sync_room_notifications(data_dir, &summary)?;

    if json {
        #[derive(Serialize)]
        struct ResetOutput {
            reset: RoomResetOutput,
            summary: RoomSummary,
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&ResetOutput { reset, summary })?
        );
        return Ok(());
    }

    println!("Reset room live activity.");
    println!("Cleared requests:    {}", reset.cleared_requests);
    println!("Cleared sessions:    {}", reset.cleared_sessions);
    println!("Cleared objects:     {}", reset.cleared_objects);
    println!("Cleared uploads:     {}", reset.cleared_uploads);
    println!("Cleared attachments: {}", reset.cleared_attachments);
    println!(
        "Preserved members:   {} active (owner: {})",
        summary.room_control.active_member_count,
        summary
            .room_control
            .owner_did
            .as_deref()
            .unwrap_or("(unseeded)")
    );
    Ok(())
}

async fn load_room_summary_with_local_context(data_dir: &Path) -> anyhow::Result<RoomSummary> {
    let mut summary = load_summary(data_dir)?;
    let runtime_did = load_room_runtime_did(data_dir).await.ok();
    let access = local_runtime_access(data_dir, runtime_did.as_deref())?;
    let hosted = load_browser_app_hosted_endpoint(data_dir, room_slug())?;
    summary.local_runtime_did = access.runtime_did;
    summary.local_runtime_role = access.member_role;
    summary.browser_access_allowed = access.browser_access_allowed;
    summary.browser_access_block_reason = access.block_reason;
    summary.canonical_hosted_guest_url = hosted.canonical_url;
    summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
    Ok(summary)
}

async fn load_room_runtime_did(data_dir: &Path) -> anyhow::Result<String> {
    if let Ok(profile) = crate::identity_cmd::load_identity_profile(data_dir).await {
        if let Some(did) = profile.did.filter(|did| !did.trim().is_empty()) {
            return Ok(did);
        }
    }

    load_persisted_room_identity_did(data_dir)
}

fn load_persisted_room_identity_did(data_dir: &Path) -> anyhow::Result<String> {
    let key_path = data_dir.join("identity").join("device.key");
    let bytes = std::fs::read(&key_path).map_err(|err| {
        anyhow::anyhow!(
            "missing room identity device key at {}: {}",
            key_path.display(),
            err
        )
    })?;
    if bytes.len() != 32 {
        anyhow::bail!(
            "room identity device key at {} has invalid length {} (expected 32)",
            key_path.display(),
            bytes.len()
        );
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    let (_signing_key, did) = elastos_identity::derive_did(&key);
    Ok(did)
}

fn print_local_room_open(opened: &elastos_server::operator_control::OperatorRoomOpen) {
    println!("Local room gateway");
    println!("Bind:       {}", opened.bind_addr);
    if let Some(room_url) = opened.room_urls.first() {
        println!("Room:       {}", room_url);
    }
    for extra in opened.room_urls.iter().skip(1) {
        println!("Also:       {}", extra);
    }
}

fn role_label(role: &RoomRole) -> &'static str {
    match role {
        RoomRole::Owner => "owner",
        RoomRole::Admin => "admin",
        RoomRole::Member => "member",
    }
}
