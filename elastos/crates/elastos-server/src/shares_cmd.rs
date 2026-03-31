pub async fn run_shares(cmd: crate::SharesCommand) -> anyhow::Result<()> {
    match cmd {
        crate::SharesCommand::List => {
            let catalog = elastos_server::shares::load_share_catalog()?;
            if catalog.channels.is_empty() {
                println!("No shares yet. Use `elastos share <path>` to publish.");
            } else {
                println!("{:<20} {:>4} {:<10} CID", "CHANNEL", "VER", "STATUS");
                for (name, channel) in &catalog.channels {
                    let did_suffix = channel
                        .author_did
                        .as_ref()
                        .or(catalog.author_did.as_ref())
                        .map(|did| {
                            if did.len() > 20 {
                                format!(" [{}...{}]", &did[..12], &did[did.len() - 3..])
                            } else {
                                format!(" [{}]", did)
                            }
                        })
                        .unwrap_or_default();
                    let prov_indicator = channel
                        .history
                        .last()
                        .and_then(|entry| entry.provenance_cid.as_ref())
                        .map(|_| " [attested]")
                        .unwrap_or("");
                    let head_indicator = channel.head_cid.as_ref().map(|_| " [head]").unwrap_or("");
                    println!(
                        "{:<20} {:>4} {:<10} {}{}{}{}",
                        name,
                        channel.latest_version,
                        channel.status,
                        channel.latest_cid,
                        did_suffix,
                        prov_indicator,
                        head_indicator
                    );
                }
            }
        }
        crate::SharesCommand::History { channel } => {
            let catalog = elastos_server::shares::load_share_catalog()?;
            match catalog.channels.get(&channel) {
                Some(share_channel) => {
                    println!("Channel: {}", channel);
                    println!("{:>4}  {:>12}  CID", "VER", "CREATED");
                    for entry in &share_channel.history {
                        let prov = entry
                            .provenance_cid
                            .as_ref()
                            .map(|cid| {
                                let truncated = if cid.len() > 16 {
                                    &cid[..16]
                                } else {
                                    cid.as_str()
                                };
                                format!("  prov:{}", truncated)
                            })
                            .unwrap_or_default();
                        println!(
                            "{:>4}  {:>12}  {}{}",
                            entry.version, entry.created_at, entry.cid, prov
                        );
                    }
                }
                None => println!("Channel '{}' not found.", channel),
            }
        }
        crate::SharesCommand::DeleteLocal { channel } => {
            let mut catalog = elastos_server::shares::load_share_catalog()?;
            if catalog.channels.remove(&channel).is_some() {
                elastos_server::shares::save_share_catalog(&catalog)?;
                println!("Removed channel '{}' from local catalog.", channel);
                println!("Note: Published content remains on IPFS.");
            } else {
                println!("Channel '{}' not found.", channel);
            }
        }
        crate::SharesCommand::Archive { channel } => {
            mutate_channel_status(
                &channel,
                elastos_server::shares::ChannelStatus::Archived,
                None,
                "Channel '{}' archived.",
                "Channel '{}' is already archived.",
            )
            .await?;
        }
        crate::SharesCommand::Unarchive { channel } => {
            mutate_channel_status(
                &channel,
                elastos_server::shares::ChannelStatus::Active,
                None,
                "Channel '{}' restored to active.",
                "Channel '{}' is not archived.",
            )
            .await?;
        }
        crate::SharesCommand::Revoke { channel, reason } => {
            mutate_channel_status(
                &channel,
                elastos_server::shares::ChannelStatus::Revoked,
                Some(reason),
                "Channel '{}' revoked.",
                "",
            )
            .await?;
            println!("Note: Published content remains on IPFS.");
        }
        crate::SharesCommand::SetDid { did } => {
            if !did.starts_with("did:key:") {
                anyhow::bail!("DID must start with 'did:key:'. Got: {}", did);
            }
            let mut catalog = elastos_server::shares::load_share_catalog()?;
            catalog.author_did = Some(did.clone());
            elastos_server::shares::save_share_catalog(&catalog)?;
            println!("Default author DID set: {}", did);
        }
        crate::SharesCommand::Head { channel } => {
            let ipfs = crate::get_ipfs_bridge().await?;
            let catalog = elastos_server::shares::load_share_catalog()?;
            match catalog.channels.get(&channel) {
                Some(share_channel) => match &share_channel.head_cid {
                    Some(head_cid) => {
                        println!("Fetching head {}...", head_cid);
                        let head_bytes = ipfs.cat(head_cid).await?;
                        match elastos_server::shares::verify_channel_head(&head_bytes) {
                            Ok(head) => {
                                let expected_did = share_channel
                                    .author_did
                                    .as_deref()
                                    .or(catalog.author_did.as_deref());
                                let trusted = match expected_did {
                                    Some(did) => head.payload.signer_did == did,
                                    None => true,
                                };
                                if trusted {
                                    println!("Head VALID (trusted)");
                                } else {
                                    println!(
                                        "Head VALID (untrusted signer: expected {})",
                                        expected_did.unwrap_or("(none)")
                                    );
                                }
                                println!("  Channel:        {}", head.payload.channel);
                                println!("  Latest CID:     {}", head.payload.latest_cid);
                                println!("  Latest version: {}", head.payload.latest_version);
                                println!("  Status:         {}", head.payload.status);
                                println!("  Signer DID:     {}", head.payload.signer_did);
                                println!("  Updated at:     {}", head.payload.updated_at);
                                if let Some(ref provenance_cid) = head.payload.provenance_cid {
                                    println!("  Provenance CID: {}", provenance_cid);
                                }
                                if let Some(ref prev_head_cid) = head.payload.prev_head_cid {
                                    println!("  Prev head CID:  {}", prev_head_cid);
                                }
                                if let Some(ref revoke_reason) = head.payload.revoke_reason {
                                    println!("  Revoke reason:  {}", revoke_reason);
                                }
                            }
                            Err(e) => {
                                println!("Head INVALID: {}", e);
                                std::process::exit(1);
                            }
                        }
                    }
                    None => println!("Channel '{}' has no published head.", channel),
                },
                None => println!("Channel '{}' not found.", channel),
            }
        }
    }

    Ok(())
}

async fn mutate_channel_status(
    channel: &str,
    new_status: elastos_server::shares::ChannelStatus,
    revoke_reason: Option<String>,
    success_message: &str,
    no_op_message: &str,
) -> anyhow::Result<()> {
    let ipfs = crate::get_ipfs_bridge().await?;
    let mut catalog = elastos_server::shares::load_share_catalog()?;

    match catalog.channels.get_mut(channel) {
        Some(share_channel) => {
            let already_no_op = match new_status {
                elastos_server::shares::ChannelStatus::Archived => {
                    share_channel.status == elastos_server::shares::ChannelStatus::Archived
                }
                elastos_server::shares::ChannelStatus::Active => {
                    share_channel.status != elastos_server::shares::ChannelStatus::Archived
                }
                elastos_server::shares::ChannelStatus::Revoked => false,
            };
            if already_no_op {
                println!("{}", no_op_message.replace("{}", channel));
                return Ok(());
            }

            share_channel.status = new_status.clone();
            if let Some(reason) = revoke_reason.as_ref() {
                share_channel.revoke_reason = Some(reason.clone());
            }
            let cid = share_channel.latest_cid.clone();
            let version = share_channel.latest_version;
            let provenance = share_channel
                .history
                .last()
                .and_then(|entry| entry.provenance_cid.clone());
            let prev_head = share_channel.head_cid.clone();

            let head_cid = elastos_server::shares::publish_channel_head(
                channel,
                &cid,
                version,
                &new_status,
                provenance.as_deref(),
                prev_head.as_deref(),
                revoke_reason.as_deref(),
                &ipfs,
            )
            .await;

            if let Some(new_head_cid) = head_cid {
                if let Some(share_channel) = catalog.channels.get_mut(channel) {
                    share_channel.head_cid = Some(new_head_cid);
                }
            }

            elastos_server::shares::save_share_catalog(&catalog)?;
            println!("{}", success_message.replace("{}", channel));
        }
        None => println!("Channel '{}' not found.", channel),
    }

    Ok(())
}
