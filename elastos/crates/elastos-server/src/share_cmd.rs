use crate::logger::cmd_share as logger;
use elastos_server::content::{
    fetch_bytes_via_provider, parse_content_object_manifest, verify_content_object_file,
    ContentObjectManifest, CONTENT_OBJECT_MANIFEST_PATH,
};
use elastos_server::shares::{
    build_share_bundle, create_provenance, derive_share_id, load_or_create_share_key,
    load_share_catalog, parse_share_uri, publish_channel_head_via_provider, save_share_catalog,
    ChannelStatus, ShareChannel, ShareEntry, ShareMeta,
};
use std::path::PathBuf;

pub async fn run_share(
    path: PathBuf,
    channel: Option<String>,
    no_attest: bool,
    no_head: bool,
    public: bool,
    public_timeout: u64,
) -> anyhow::Result<()> {
    let content_registry = crate::get_content_registry().await?;
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
    let cid = elastos_server::content::publish_directory_via_provider_with_kind(
        &content_registry,
        bundle.path(),
        "share",
        Some(&share_id),
        author_did.as_deref(),
    )
    .await?;

    let signing_key = load_or_create_share_key()?;
    let provenance_cid = if no_attest {
        None
    } else {
        let prov_bytes = create_provenance(&cid, &meta.content_digest, &signing_key)?;
        Some(
            elastos_server::content::publish_bytes_via_provider(
                &content_registry,
                "provenance.json",
                &prov_bytes,
                Some(&share_id),
                author_did.as_deref(),
            )
            .await?,
        )
    };

    let head_cid = if no_head {
        None
    } else {
        publish_channel_head_via_provider(
            &content_registry,
            &share_id,
            &cid,
            meta.version,
            &ChannelStatus::Active,
            provenance_cid.as_deref(),
            prev_head_cid.as_deref(),
            None,
            author_did.as_deref(),
            &signing_key,
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
    println!(
        "  Preview local:   elastos open elastos://{} --browser",
        cid
    );
    println!(
        "  Open elsewhere:  after installing Documents and the content backend, run `elastos open elastos://{} --browser`",
        cid
    );
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

    let ipfs = crate::get_operator_ipfs_bridge().await?;
    let (tunnel, public_url) =
        crate::start_operator_public_share_tunnel(&ipfs, &cid, public_timeout)
            .await
            .map_err(|e| anyhow::anyhow!("share succeeded, but --public failed: {}", e))?;
    println!("  Public link:     {}", public_url);
    println!();
    println!("  Public link is live. Press Ctrl+C to stop public sharing.");

    tokio::signal::ctrl_c().await?;
    if let Err(err) = tunnel.shutdown().await {
        logger::warn!("failed to stop public share cleanly: {}", err);
    }
    Ok(())
}

pub async fn run_open(uri: String, browser: bool, port: Option<u16>) -> anyhow::Result<()> {
    if let Some(subpath) = crate::site_cmd::parse_public_site_uri(&uri) {
        let addr = crate::choose_local_open_addr(port)?;
        return crate::site_cmd::open_public_site(subpath, addr, browser).await;
    }

    let cid = parse_share_uri(&uri)?;
    let content_registry = crate::get_content_registry().await?;
    if let Ok(object_manifest_bytes) =
        fetch_bytes_via_provider(&content_registry, &cid, Some(CONTENT_OBJECT_MANIFEST_PATH)).await
    {
        let object_manifest = parse_content_object_manifest(&cid, &object_manifest_bytes)?;
        if open_release_object_if_applicable(&content_registry, &cid, &object_manifest, browser)
            .await?
        {
            return Ok(());
        }
    }

    let catalog = load_share_catalog()?;

    let share_meta = elastos_server::content::fetch_bytes_via_provider(
        &content_registry,
        &cid,
        Some("_share.json"),
    )
    .await
    .ok()
    .and_then(|bytes| serde_json::from_slice::<ShareMeta>(&bytes).ok());
    if let Some(meta) = share_meta.as_ref() {
        crate::print_share_open_warnings(&content_registry, &catalog, &cid, meta).await;
    }

    let capsule_dir =
        elastos_server::content::prepare_capsule_from_content_provider(&content_registry, &cid)
            .await?;
    let addr = crate::choose_local_open_addr(port)?;
    let runtime = crate::create_runtime("/tmp/elastos/storage").await?;
    crate::serve_web_capsule(runtime, capsule_dir, &addr, browser, None).await
}

async fn open_release_object_if_applicable(
    registry: &elastos_runtime::provider::ProviderRegistry,
    cid: &str,
    manifest: &ContentObjectManifest,
    browser: bool,
) -> anyhow::Result<bool> {
    if manifest.kind != "release" {
        return Ok(false);
    }

    if browser {
        println!("Release objects are metadata; showing a terminal summary instead of opening a browser.");
        println!();
    }

    print_release_object_summary(registry, cid, manifest).await?;
    Ok(true)
}

async fn print_release_object_summary(
    registry: &elastos_runtime::provider::ProviderRegistry,
    cid: &str,
    manifest: &ContentObjectManifest,
) -> anyhow::Result<()> {
    println!("Release object");
    println!("  Address:   elastos://{cid}");
    if let Some(object_did) = &manifest.object_did {
        println!("  Object:    {object_did}");
    }
    if let Some(publisher_did) = &manifest.publisher_did {
        println!("  Publisher: {publisher_did}");
    }

    if let Some(file) = manifest
        .files
        .iter()
        .find(|file| file.path == "release.json")
    {
        let bytes =
            elastos_server::content::fetch_bytes_via_provider(registry, cid, Some("release.json"))
                .await?;
        verify_content_object_file(cid, file, &bytes)?;
        let (envelope, signer_did) =
            elastos_server::crypto::verify_signed_json_envelope_against_dids(
                &bytes,
                "elastos.release.v1",
                &[],
            )?;
        let payload = &envelope["payload"];
        println!("  Type:      release manifest");
        print_json_field("  Version:   ", payload.get("version"));
        print_json_field("  Channel:   ", payload.get("channel"));
        println!("  Signer:    {signer_did}");
        if let Some(platforms) = payload.get("platforms").and_then(|value| value.as_object()) {
            let names = platforms.keys().cloned().collect::<Vec<_>>();
            println!("  Platforms: {}", names.join(", "));
        }
    } else if let Some(file) = manifest
        .files
        .iter()
        .find(|file| file.path == "release-head.json")
    {
        let bytes = elastos_server::content::fetch_bytes_via_provider(
            registry,
            cid,
            Some("release-head.json"),
        )
        .await?;
        verify_content_object_file(cid, file, &bytes)?;
        let (envelope, signer_did) =
            elastos_server::crypto::verify_signed_json_envelope_against_dids(
                &bytes,
                "elastos.release.head.v1",
                &[],
            )?;
        let payload = &envelope["payload"];
        println!("  Type:      release head");
        print_json_field("  Version:   ", payload.get("version"));
        print_json_field("  Channel:   ", payload.get("channel"));
        print_json_field("  Release:   ", payload.get("latest_release_cid"));
        print_json_field("  Object:    ", payload.get("release_object_cid"));
        println!("  Signer:    {signer_did}");
    } else if manifest.files.iter().any(|file| file.path == "install.sh") {
        println!("  Type:      installer script");
    } else {
        println!("  Type:      release-linked object");
    }

    if !manifest.links.is_empty() {
        println!("  Links:     {} linked CID(s)", manifest.links.len());
        for rel in ["head", "head.object", "release", "release.object"] {
            if let Some(link) = manifest.links.iter().find(|link| link.rel == rel) {
                println!("    {rel}: elastos://{}", link.cid);
            }
        }
    }

    println!();
    println!("This is release metadata, not a launchable app capsule.");
    println!("Use `elastos update` from a trusted source to install it.");
    Ok(())
}

fn print_json_field(label: &str, value: Option<&serde_json::Value>) {
    if let Some(value) = value.and_then(|value| value.as_str()) {
        if !value.trim().is_empty() {
            println!("{label}{value}");
        }
    }
}
