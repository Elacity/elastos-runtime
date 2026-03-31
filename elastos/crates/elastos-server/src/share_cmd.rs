use std::path::PathBuf;

use elastos_server::shares::{
    build_share_bundle, create_provenance, derive_share_id, load_or_create_share_key,
    load_share_catalog, parse_share_uri, publish_channel_head, save_share_catalog, ChannelStatus,
    ShareChannel, ShareEntry, ShareMeta,
};

pub async fn run_share(
    path: PathBuf,
    channel: Option<String>,
    no_attest: bool,
    no_head: bool,
    public: bool,
    public_timeout: u64,
) -> anyhow::Result<()> {
    let ipfs = crate::get_ipfs_bridge().await?;
    let mut catalog = load_share_catalog()?;
    let share_id = derive_share_id(&path, channel.as_deref())?;
    let existing = catalog.channels.get(&share_id);
    let version = existing.map(|ch| ch.latest_version + 1).unwrap_or(1);
    let prev_cid =
        existing.and_then(|ch| (!ch.latest_cid.is_empty()).then(|| ch.latest_cid.clone()));
    let prev_head_cid = existing.and_then(|ch| ch.head_cid.clone());
    let author_did = existing
        .and_then(|ch| ch.author_did.clone())
        .or_else(|| catalog.author_did.clone());

    let (bundle, meta) = build_share_bundle(
        &path,
        &share_id,
        version,
        prev_cid.as_deref(),
        author_did.as_deref(),
    )?;

    println!("Sharing '{}'...", path.display());
    let cid = ipfs.add_directory_from_path(bundle.path()).await?;

    let signing_key = load_or_create_share_key()?;
    let provenance_cid = if no_attest {
        None
    } else {
        let prov_bytes = create_provenance(&cid, &meta.content_digest, &signing_key)?;
        Some(ipfs.add_bytes(&prov_bytes, "provenance.json").await?)
    };

    let head_cid = if no_head {
        None
    } else {
        publish_channel_head(
            &share_id,
            &cid,
            meta.version,
            &ChannelStatus::Active,
            provenance_cid.as_deref(),
            prev_head_cid.as_deref(),
            None,
            &ipfs,
        )
        .await
    };

    let channel_entry = catalog
        .channels
        .entry(share_id.clone())
        .or_insert_with(ShareChannel::default);
    channel_entry.latest_cid = cid.clone();
    channel_entry.latest_version = meta.version;
    channel_entry.updated_at = meta.created_at;
    channel_entry.status = ChannelStatus::Active;
    channel_entry.author_did = author_did.clone();
    channel_entry.head_cid = head_cid.clone();
    channel_entry.history.push(ShareEntry {
        cid: cid.clone(),
        version: meta.version,
        created_at: meta.created_at,
        content_digest: Some(meta.content_digest.clone()),
        provenance_cid: provenance_cid.clone(),
    });
    save_share_catalog(&catalog)?;

    println!();
    println!("Shared: elastos://{}", cid);
    println!();
    println!("  Open locally:    elastos open elastos://{}", cid);
    println!(
        "  Public link:     run `elastos share --public {}`",
        path.display()
    );
    if let Some(pcid) = provenance_cid {
        println!("  Provenance:      {}", pcid);
    }
    println!();
    println!("  Channel: {}  Version: {}", share_id, meta.version);

    if !public {
        return Ok(());
    }

    let (tunnel, public_url) = crate::start_public_share_tunnel(&ipfs, &cid, public_timeout)
        .await
        .map_err(|e| anyhow::anyhow!("share succeeded, but --public failed: {}", e))?;
    println!("  Public link:     {}", public_url);
    println!();
    println!("  Public link is live. Press Ctrl+C to stop public sharing.");

    tokio::signal::ctrl_c().await?;
    if let Err(err) = tunnel.shutdown().await {
        eprintln!("Warning: failed to stop public share cleanly: {}", err);
    }
    Ok(())
}

pub async fn run_open(uri: String, browser: bool, port: Option<u16>) -> anyhow::Result<()> {
    if let Some(subpath) = crate::site_cmd::parse_public_site_uri(&uri) {
        let addr = crate::choose_local_open_addr(port)?;
        return crate::site_cmd::open_public_site(subpath, addr, browser).await;
    }

    let cid = parse_share_uri(&uri)?;
    let ipfs = crate::get_ipfs_bridge().await?;
    let catalog = load_share_catalog()?;

    let share_meta = ipfs
        .cat_with_path(&cid, "_share.json")
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ShareMeta>(&bytes).ok());
    if let Some(meta) = share_meta.as_ref() {
        crate::print_share_open_warnings(&ipfs, &catalog, &cid, meta).await;
    }

    let capsule_dir = elastos_server::ipfs::prepare_capsule_from_cid(&ipfs, &cid).await?;
    let addr = crate::choose_local_open_addr(port)?;
    let runtime = crate::create_runtime("/tmp/elastos/storage").await?;
    crate::serve_web_capsule(runtime, capsule_dir, &addr, browser, None).await
}
