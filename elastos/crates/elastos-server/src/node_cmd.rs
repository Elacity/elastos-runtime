use std::path::Path;

use anyhow::Context;

use elastos_server::operator_control::{
    gather_local_node_status, remove_peer, request_remote_room_approve, request_remote_room_deny,
    request_remote_room_open, request_remote_room_read, request_remote_status,
    request_remote_update_apply, request_remote_update_check, supported_actions, upsert_peer,
    OperatorPeer, OperatorRoomOpen,
};
use elastos_server::room_service::{ApprovalOutcome, DenyOutcome, RoomSummary};
use elastos_server::sources::default_data_dir;

pub async fn run_node(cmd: crate::NodeCommand) -> anyhow::Result<()> {
    let data_dir = default_data_dir();

    match cmd {
        crate::NodeCommand::Info { json } => run_info(&data_dir, json).await,
        crate::NodeCommand::Peer(cmd) => run_peer(&data_dir, cmd).await,
        crate::NodeCommand::Room(cmd) => run_room(&data_dir, cmd).await,
        crate::NodeCommand::Status { peer, json } => run_status(&data_dir, &peer, json).await,
        crate::NodeCommand::Update {
            peer,
            check,
            apply,
            yes,
            json,
        } => run_update(&data_dir, &peer, check, apply, yes, json).await,
    }
}

async fn run_room(data_dir: &Path, cmd: crate::NodeRoomCommand) -> anyhow::Result<()> {
    match cmd {
        crate::NodeRoomCommand::Show { peer, json } => {
            let (peer_config, _) = require_operator_target(data_dir, &peer).await?;
            let summary = request_remote_room_read(data_dir, &peer_config).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
                return Ok(());
            }
            print_remote_room_summary(&peer, &summary);
            Ok(())
        }
        crate::NodeRoomCommand::Pending { peer, json } => {
            let (peer_config, _) = require_operator_target(data_dir, &peer).await?;
            let summary = request_remote_room_read(data_dir, &peer_config).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&summary.pending_requests)?
                );
                return Ok(());
            }
            print_remote_room_pending(&peer, &summary);
            Ok(())
        }
        crate::NodeRoomCommand::Approve {
            peer,
            request_id,
            json,
        } => {
            let (peer_config, _) = require_operator_target(data_dir, &peer).await?;
            let outcome =
                request_remote_room_approve(data_dir, &peer_config, request_id.as_deref()).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
                return Ok(());
            }
            print_remote_room_approve(&peer, request_id.as_deref(), outcome);
            Ok(())
        }
        crate::NodeRoomCommand::Deny {
            peer,
            request_id,
            reason,
            json,
        } => {
            let (peer_config, _) = require_operator_target(data_dir, &peer).await?;
            let reason = reason.unwrap_or_else(|| "Denied from remote operator CLI".to_string());
            let outcome =
                request_remote_room_deny(data_dir, &peer_config, request_id.as_deref(), &reason)
                    .await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
                return Ok(());
            }
            print_remote_room_deny(&peer, request_id.as_deref(), outcome);
            Ok(())
        }
        crate::NodeRoomCommand::Open { peer, addr, json } => {
            let (peer_config, _) = require_operator_target(data_dir, &peer).await?;
            let opened = request_remote_room_open(data_dir, &peer_config, &addr).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&opened)?);
                return Ok(());
            }
            print_remote_room_open(&peer, &opened);
            Ok(())
        }
    }
}

async fn run_info(data_dir: &Path, json: bool) -> anyhow::Result<()> {
    let status = gather_local_node_status(data_dir).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("Local operator node");
    println!("DID:            {}", status.did);
    println!(
        "Runtime:        {}",
        if status.runtime_running {
            "running"
        } else {
            "not running"
        }
    );
    if let Some(version) = status.runtime_version.as_deref() {
        println!("Version:        {}", version);
    }
    if let Some(kind) = status.runtime_kind.as_deref() {
        println!("Runtime kind:   {}", kind);
    }
    if let Some(ticket) = status.connect_ticket.as_deref() {
        println!("Connect ticket: {}", ticket);
    } else if status.runtime_running {
        println!("Connect ticket: unavailable");
    }
    if let Some(source) = status.source.as_ref() {
        println!(
            "Trusted source: {} ({}, channel {})",
            source.name,
            display_version(&source.installed_version),
            source.channel
        );
    }
    if let Some(note) = status.note.as_deref() {
        println!("Note:           {}", note);
    }
    Ok(())
}

async fn run_peer(data_dir: &Path, cmd: crate::NodePeerCommand) -> anyhow::Result<()> {
    match cmd {
        crate::NodePeerCommand::Add {
            did,
            label,
            ticket,
            allow,
            json,
        } => {
            let peer = upsert_peer(
                data_dir,
                OperatorPeer {
                    did,
                    label: label.unwrap_or_default(),
                    connect_ticket: ticket.unwrap_or_default(),
                    allow,
                },
            )?;
            if json {
                println!("{}", serde_json::to_string_pretty(&peer)?);
                return Ok(());
            }
            print_peer_summary("Saved operator peer.", &peer);
            Ok(())
        }
        crate::NodePeerCommand::List { json } => {
            let config = elastos_server::operator_control::load_operator_control(data_dir)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&config.peers)?);
                return Ok(());
            }
            if config.peers.is_empty() {
                println!("No operator peers configured.");
                return Ok(());
            }
            for peer in &config.peers {
                print_peer_summary("Peer", peer);
                println!();
            }
            Ok(())
        }
        crate::NodePeerCommand::Remove { did, json } => {
            let removed = remove_peer(data_dir, &did)?;
            let peer =
                removed.with_context(|| format!("operator peer '{}' is not configured", did))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&peer)?);
                return Ok(());
            }
            print_peer_summary("Removed operator peer.", &peer);
            Ok(())
        }
    }
}

async fn run_status(data_dir: &Path, did: &str, json: bool) -> anyhow::Result<()> {
    let peer = load_peer(data_dir, did)?;
    let status = request_remote_status(data_dir, &peer).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("Remote node");
    println!("Peer DID:       {}", status.did);
    println!(
        "Runtime:        {}",
        if status.runtime_running {
            "running"
        } else {
            "not running"
        }
    );
    if let Some(version) = status.runtime_version.as_deref() {
        println!("Version:        {}", version);
    }
    if let Some(kind) = status.runtime_kind.as_deref() {
        println!("Runtime kind:   {}", kind);
        if kind != "operator" {
            println!(
                "Note:           Target DID is currently served by `{}`. Start `elastos serve` on the target for strict operator control.",
                kind
            );
        }
    }
    if let Some(source) = status.source.as_ref() {
        println!(
            "Trusted source: {} ({}, channel {})",
            source.name,
            display_version(&source.installed_version),
            source.channel
        );
    }
    if let Some(peer_count) = status.peer_count {
        println!("Carrier peers:  {}", peer_count);
    }
    if !status.running_capsules.is_empty() {
        println!("Capsules:       {}", status.running_capsules.join(", "));
    }
    if let Some(ticket) = status.connect_ticket.as_deref() {
        println!("Connect ticket: {}", ticket);
    }
    if let Some(note) = status.note.as_deref() {
        println!("Note:           {}", note);
    }
    Ok(())
}

async fn run_update(
    data_dir: &Path,
    did: &str,
    check: bool,
    apply: bool,
    yes: bool,
    json: bool,
) -> anyhow::Result<()> {
    let (peer, _status) = require_operator_target(data_dir, did).await?;
    match (check, apply) {
        (true, false) => {
            let update = request_remote_update_check(data_dir, &peer).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&update)?);
                return Ok(());
            }

            println!("Remote update check");
            println!("Peer DID:        {}", did);
            println!("Trusted source:  {}", update.source_name);
            println!("Channel:         {}", update.channel);
            println!(
                "Installed:       {}",
                display_version(&update.current_version)
            );
            println!(
                "Latest:          {}",
                display_version(&update.latest_version)
            );
            println!(
                "Update available: {}",
                if update.update_available { "yes" } else { "no" }
            );
            println!("Discovery:       {}", update.discovery);
            if let Some(gateway) = update.working_gateway.as_deref() {
                println!("Gateway:         {}", gateway);
            }
            Ok(())
        }
        (false, true) => {
            if !yes {
                anyhow::bail!(
                    "Remote update apply is mutating. Re-run with `elastos node update --peer <did> --apply --yes`."
                );
            }

            let update = request_remote_update_apply(data_dir, &peer).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&update)?);
                return Ok(());
            }

            println!("Remote update apply");
            println!("Peer DID:         {}", did);
            println!("Trusted source:   {}", update.source_name);
            println!("Channel:          {}", update.channel);
            println!(
                "Previous:         {}",
                display_version(&update.previous_version)
            );
            println!(
                "Installed:        {}",
                display_version(&update.installed_version)
            );
            println!(
                "Changed:          {}",
                if update.changed { "yes" } else { "no" }
            );
            println!(
                "Runtime running:  {}",
                if update.runtime_running { "yes" } else { "no" }
            );
            if let Some(version) = update.runtime_version.as_deref() {
                println!("Runtime version:  {}", version);
            }
            println!(
                "Restart required: {}",
                if update.restart_required { "yes" } else { "no" }
            );
            if let Some(note) = update.note.as_deref() {
                println!("Note:             {}", note);
            }
            Ok(())
        }
        (false, false) => {
            anyhow::bail!("Choose one explicit operator action: `--check` or `--apply --yes`.")
        }
        (true, true) => anyhow::bail!("Choose only one of `--check` or `--apply`."),
    }
}

async fn require_operator_target(
    data_dir: &Path,
    did: &str,
) -> anyhow::Result<(
    OperatorPeer,
    elastos_server::operator_control::OperatorNodeStatus,
)> {
    let peer = load_peer(data_dir, did)?;
    let status = request_remote_status(data_dir, &peer).await?;
    let runtime_kind = status.runtime_kind.as_deref().unwrap_or("unknown");
    if runtime_kind != "operator" {
        anyhow::bail!(
            "operator peer '{}' is currently served by runtime kind '{}'. Start `elastos serve` on the target and retry.",
            did,
            runtime_kind
        );
    }
    if !status.runtime_running {
        anyhow::bail!(
            "operator peer '{}' is not running. Start `elastos serve` on the target and retry.",
            did
        );
    }
    Ok((peer, status))
}

fn load_peer(data_dir: &Path, did: &str) -> anyhow::Result<OperatorPeer> {
    let config = elastos_server::operator_control::load_operator_control(data_dir)?;
    config
        .peers
        .into_iter()
        .find(|peer| peer.did == did)
        .with_context(|| {
            format!(
                "operator peer '{}' is not configured. Add it with:\n  elastos node peer add --did {} --ticket <connect-ticket>\nSupported allow actions: {}",
                did,
                did,
                supported_actions().join(", ")
            )
        })
}

fn display_version(version: &str) -> &str {
    if version.trim().is_empty() {
        "unknown"
    } else {
        version
    }
}

fn print_peer_summary(prefix: &str, peer: &OperatorPeer) {
    println!("{}", prefix);
    println!("DID:            {}", peer.did);
    if !peer.label.is_empty() {
        println!("Label:          {}", peer.label);
    }
    println!(
        "Route:          {}",
        if peer.connect_ticket.is_empty() {
            "none"
        } else {
            "ticket configured"
        }
    );
    println!(
        "Allow:          {}",
        if peer.allow.is_empty() {
            "none".to_string()
        } else {
            peer.allow.join(", ")
        }
    );
}

fn print_remote_room_summary(peer_did: &str, summary: &RoomSummary) {
    println!("Remote room");
    println!("Peer DID:       {}", peer_did);
    println!("Room:           {}", summary.room_control.title);
    println!("Slug:           {}", summary.room_slug);
    println!(
        "Root:           {}",
        elastos_server::room_service::room_root_uri()
    );
    if let Some(role) = summary.local_runtime_role.as_ref() {
        println!("Role:           {}", role_label(role));
    } else {
        println!("Role:           remote runtime is not a room member");
    }
    println!(
        "Runtime DID:    {}",
        summary
            .local_runtime_did
            .as_deref()
            .unwrap_or("(local DID unavailable)")
    );
    println!(
        "Members:        {} total, {} active, {} admin",
        summary.room_control.member_count,
        summary.room_control.active_member_count,
        summary.room_control.admin_count
    );
    println!(
        "Participants:   {} in room",
        summary.active_participants.len()
    );
    println!(
        "Pairing:        {} pending request(s)",
        summary.pending_requests.len()
    );
    if let Some(url) = summary.canonical_hosted_guest_url.as_deref() {
        println!("Hosted URL:     {}", url);
    }
    if let Some(url) = summary.ephemeral_hosted_guest_url.as_deref() {
        println!("Quick URL:      {}", url);
    }
    if let Some(status) = summary.transport.status.as_deref() {
        println!("Transport:      {}", status);
    }
}

fn print_remote_room_pending(peer_did: &str, summary: &RoomSummary) {
    if summary.pending_requests.is_empty() {
        println!("No pending room browser requests on {}.", peer_did);
        return;
    }
    println!(
        "Pending room browser requests on {}: {}",
        peer_did,
        summary.pending_requests.len()
    );
    for request in &summary.pending_requests {
        println!(
            "- {} on {} (id: {}, requested: {})",
            request.display_name, request.device_label, request.request_id, request.requested_at
        );
    }
}

fn print_remote_room_approve(
    peer_did: &str,
    requested_id: Option<&str>,
    outcome: Option<ApprovalOutcome>,
) {
    match outcome {
        Some(outcome) => {
            println!("Approved remote room browser pairing.");
            println!("Peer DID:       {}", peer_did);
            println!("Request:        {}", outcome.request_id);
            println!(
                "Browser:        {} on {}",
                outcome.display_name, outcome.device_label
            );
            println!("Expires:        {}", outcome.expires_at);
        }
        None => {
            if let Some(request_id) = requested_id {
                println!(
                    "That remote room browser request is no longer pending: {}",
                    request_id
                );
            } else {
                println!("No pending remote room browser requests.");
            }
        }
    }
}

fn print_remote_room_deny(
    peer_did: &str,
    requested_id: Option<&str>,
    outcome: Option<DenyOutcome>,
) {
    match outcome {
        Some(outcome) => {
            println!("Denied remote room browser pairing.");
            println!("Peer DID:       {}", peer_did);
            println!("Request:        {}", outcome.request_id);
            println!(
                "Browser:        {} on {}",
                outcome.display_name, outcome.device_label
            );
            println!("Reason:         {}", outcome.reason);
        }
        None => {
            if let Some(request_id) = requested_id {
                println!(
                    "That remote room browser request is no longer pending: {}",
                    request_id
                );
            } else {
                println!("No pending remote room browser requests.");
            }
        }
    }
}

fn print_remote_room_open(peer_did: &str, opened: &OperatorRoomOpen) {
    println!("Remote room gateway");
    println!("Peer DID:       {}", peer_did);
    println!("Bind:           {}", opened.bind_addr);
    if let Some(room_url) = opened.room_urls.first() {
        println!("Room:           {}", room_url);
    }
    for extra in opened.room_urls.iter().skip(1) {
        println!("Also:           {}", extra);
    }
}

fn role_label(role: &elastos_server::room_service::RoomRole) -> &'static str {
    match role {
        elastos_server::room_service::RoomRole::Owner => "owner",
        elastos_server::room_service::RoomRole::Admin => "admin",
        elastos_server::room_service::RoomRole::Member => "member",
    }
}
