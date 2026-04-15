use std::path::Path;

use anyhow::Context;

use elastos_server::operator_control::{
    gather_local_node_status, remove_peer, request_remote_status, request_remote_update_apply,
    request_remote_update_check, supported_actions, upsert_peer, OperatorPeer,
};
use elastos_server::sources::default_data_dir;

pub async fn run_node(cmd: crate::NodeCommand) -> anyhow::Result<()> {
    let data_dir = default_data_dir();

    match cmd {
        crate::NodeCommand::Info { json } => run_info(&data_dir, json).await,
        crate::NodeCommand::Peer(cmd) => run_peer(&data_dir, cmd).await,
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
    let peer = load_peer(data_dir, did)?;
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
