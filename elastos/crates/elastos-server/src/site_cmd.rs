use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use elastos_common::localhost::{
    edge_binding_path, edge_release_channel_path, edge_release_channels_dir, edge_site_head_path,
    edge_site_history_dir, my_website_root_path, publisher_site_release_path,
    publisher_site_releases_dir, rooted_localhost_fs_path, sanitize_edge_state_name,
    MY_WEBSITE_URI,
};
use elastos_runtime::provider::{BridgeProviderConfig, ProviderBridge};
use elastos_server::crypto::domain_separated_sign;
use elastos_server::ipfs::IpfsBridge;
use elastos_server::shares::load_or_create_share_key;
use elastos_server::sources::default_data_dir;

const DEFAULT_SITE_MODE: &str = "local";
const DEFAULT_SITE_ADDR: &str = "127.0.0.1:8081";
const SITE_HEAD_DOMAIN: &str = "elastos.site.head.v1";
const SITE_RELEASE_DOMAIN: &str = "elastos.site.release.v1";
const SITE_CHANNEL_DOMAIN: &str = "elastos.site.channel.v1";

#[derive(Debug, Serialize)]
struct SiteDomainBinding {
    domain: String,
    target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SiteHeadPayload {
    schema: String,
    target: String,
    #[serde(default)]
    bundle_cid: Option<String>,
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    channel_name: Option<String>,
    content_digest: String,
    entry_count: u64,
    total_bytes: u64,
    activated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SiteHeadEnvelope {
    payload: SiteHeadPayload,
    signature: String,
    signer_did: String,
}

#[derive(Debug)]
struct SiteDigest {
    digest_hex: String,
    entry_count: u64,
    total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SiteReleaseRecord {
    schema: String,
    target: String,
    release_name: String,
    bundle_cid: String,
    content_digest: String,
    entry_count: u64,
    total_bytes: u64,
    published_at: u64,
}

#[derive(Debug, Serialize)]
struct SiteReleaseEntry {
    release_name: String,
    bundle_cid: String,
    content_digest: String,
    published_at: u64,
    active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SiteReleaseChannelRecord {
    schema: String,
    target: String,
    channel_name: String,
    release_name: String,
    bundle_cid: String,
    promoted_at: u64,
}

#[derive(Debug, Serialize)]
struct SiteReleaseChannelEntry {
    channel_name: String,
    release_name: String,
    bundle_cid: String,
    promoted_at: u64,
    active: bool,
}

#[derive(Debug, Serialize)]
struct SiteHeadHistoryEntry {
    release_name: Option<String>,
    channel_name: Option<String>,
    bundle_cid: Option<String>,
    content_digest: String,
    activated_at: u64,
    signer_did: String,
    current: bool,
}

#[derive(Debug, Default, Deserialize)]
struct TunnelStatus {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    last_log: Option<String>,
}

struct TunnelBridge {
    bridge: ProviderBridge,
}

pub(crate) struct PublicTunnelSession {
    target: String,
    public_url: String,
    tunnel: TunnelBridge,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct SiteStatus {
    #[serde(default)]
    pub(crate) local_url: Option<String>,
    #[serde(default)]
    pub(crate) reused: Option<bool>,
}

struct SiteBridge {
    bridge: ProviderBridge,
}

pub(crate) struct LocalSitePreviewSession {
    site_path: PathBuf,
    bridge: SiteBridge,
}

impl SiteBridge {
    async fn start(&self, addr: &str) -> anyhow::Result<SiteStatus> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "start",
                "addr": addr,
            }))
            .await
            .map_err(|e| anyhow::anyhow!("site-provider bridge error: {}", e))?;
        parse_site_status_response(resp, "start")
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.bridge
            .send_raw(&serde_json::json!({ "op": "shutdown" }))
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("site-provider shutdown failed: {}", e))
    }
}

impl LocalSitePreviewSession {
    async fn ensure_started(&mut self, addr: &str) -> anyhow::Result<SiteStatus> {
        validate_site_path(&self.site_path)?;
        self.bridge.start(addr).await
    }

    async fn shutdown(self) -> anyhow::Result<()> {
        self.bridge.shutdown().await
    }
}

pub(crate) async fn ensure_local_site_preview(
    session: &mut Option<LocalSitePreviewSession>,
    addr: &str,
) -> anyhow::Result<SiteStatus> {
    if session.is_none() {
        let site_path = site_root_dir();
        validate_site_path(&site_path)?;
        let bridge = get_site_bridge(&site_path).await?;
        *session = Some(LocalSitePreviewSession { site_path, bridge });
    }

    session
        .as_mut()
        .expect("site preview session initialized")
        .ensure_started(addr)
        .await
}

pub(crate) async fn shutdown_local_site_preview(
    session: &mut Option<LocalSitePreviewSession>,
) -> anyhow::Result<()> {
    if let Some(session) = session.take() {
        session.shutdown().await?;
    }
    Ok(())
}

pub(crate) async fn ensure_public_tunnel(
    session: &mut Option<PublicTunnelSession>,
    target: &str,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let needs_start = match session.as_mut() {
        Some(current) if current.target == target => {
            let status = current.tunnel.status().await.unwrap_or_default();
            !(status.running || status.url.is_some())
        }
        Some(_) => true,
        None => true,
    };

    if needs_start {
        shutdown_public_tunnel(session).await?;
        let tunnel = get_tunnel_bridge().await?;
        let public_url = start_public_tunnel(&tunnel, target, timeout_secs).await?;
        *session = Some(PublicTunnelSession {
            target: target.to_string(),
            public_url: public_url.clone(),
            tunnel,
        });
    }

    session
        .as_ref()
        .map(|current| current.public_url.clone())
        .ok_or_else(|| anyhow::anyhow!("public tunnel did not start"))
}

pub(crate) async fn shutdown_public_tunnel(
    session: &mut Option<PublicTunnelSession>,
) -> anyhow::Result<()> {
    if let Some(session) = session.take() {
        session.tunnel.shutdown().await?;
    }
    Ok(())
}

impl TunnelBridge {
    async fn start(&self, target: &str) -> anyhow::Result<TunnelStatus> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({
                "op": "start",
                "target": target,
            }))
            .await
            .map_err(|e| anyhow::anyhow!("tunnel-provider bridge error: {}", e))?;
        parse_tunnel_status_response(resp, "start")
    }

    async fn status(&self) -> anyhow::Result<TunnelStatus> {
        let resp = self
            .bridge
            .send_raw(&serde_json::json!({ "op": "status" }))
            .await
            .map_err(|e| anyhow::anyhow!("tunnel-provider bridge error: {}", e))?;
        parse_tunnel_status_response(resp, "status")
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.bridge
            .send_raw(&serde_json::json!({ "op": "stop" }))
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("tunnel-provider shutdown failed: {}", e))
    }
}

pub(crate) fn parse_public_site_uri(uri: &str) -> Option<String> {
    if uri == MY_WEBSITE_URI {
        return Some(String::new());
    }
    let rest = uri.strip_prefix(&format!("{}/", MY_WEBSITE_URI))?;
    Some(rest.trim_matches('/').to_string())
}

pub(crate) async fn open_public_site(
    subpath: String,
    addr: String,
    browser: bool,
) -> anyhow::Result<()> {
    serve_site_impl(
        DEFAULT_SITE_MODE,
        &addr,
        None,
        browser,
        60,
        if subpath.is_empty() {
            None
        } else {
            Some(subpath.as_str())
        },
    )
    .await
}

pub(crate) async fn run(cmd: crate::SiteCommand) -> anyhow::Result<()> {
    match cmd {
        crate::SiteCommand::Stage { source } => stage_site(&source)?,
        crate::SiteCommand::Path => print_site_path()?,
        crate::SiteCommand::Publish { target, release } => {
            publish_site(target.as_deref(), release.as_deref()).await?;
        }
        crate::SiteCommand::Releases { target, json } => {
            print_site_releases(target.as_deref(), json)?;
        }
        crate::SiteCommand::Channels { target, json } => {
            print_site_channels(target.as_deref(), json)?;
        }
        crate::SiteCommand::History { target, json } => {
            print_site_history(target.as_deref(), json)?;
        }
        crate::SiteCommand::Activate {
            target,
            release,
            channel,
        } => {
            activate_site(target.as_deref(), release.as_deref(), channel.as_deref()).await?;
        }
        crate::SiteCommand::Rollback { revision, target } => {
            rollback_site(revision.as_deref(), target.as_deref())?;
        }
        crate::SiteCommand::BindDomain { domain, target } => {
            bind_domain(&domain, target.as_deref())?;
        }
        crate::SiteCommand::Promote {
            channel,
            release,
            target,
        } => {
            promote_site_release(&channel, &release, target.as_deref())?;
        }
        crate::SiteCommand::Serve {
            mode,
            addr,
            domain,
            browser,
            public_timeout,
        } => {
            serve_site_impl(
                &mode,
                &addr,
                domain.as_deref(),
                browser,
                public_timeout,
                None,
            )
            .await?;
        }
    }

    Ok(())
}

fn site_root_dir() -> PathBuf {
    my_website_root_path(&default_data_dir())
}

fn ensure_site_root() -> anyhow::Result<()> {
    let root = site_root_dir();
    fs::create_dir_all(&root)?;
    Ok(())
}

fn validate_site_path(site_path: &Path) -> anyhow::Result<()> {
    if !site_path.exists() {
        anyhow::bail!(
            "site not staged at {}. Run: elastos site stage <dir>",
            site_path.display()
        );
    }

    if !site_path.join("index.html").exists() {
        anyhow::bail!("site root {} is missing index.html", site_path.display());
    }

    Ok(())
}

fn stage_site(source: &Path) -> anyhow::Result<()> {
    ensure_site_root()?;

    if !source.exists() {
        anyhow::bail!("source does not exist: {}", source.display());
    }
    if !source.is_dir() {
        anyhow::bail!(
            "site source must be a directory containing index.html: {}",
            source.display()
        );
    }
    if !source.join("index.html").exists() {
        anyhow::bail!("site source is missing index.html: {}", source.display());
    }

    let dest = site_root_dir();
    atomic_copy_dir(source, &dest)?;

    println!("Staged MyWebSite");
    println!("  Local root: {}", MY_WEBSITE_URI);
    println!("  Path:       {}", dest.display());
    println!("  Serve:      elastos site serve");
    Ok(())
}

fn print_site_path() -> anyhow::Result<()> {
    let path = site_root_dir();
    println!("Site root:  {}", MY_WEBSITE_URI);
    println!("Path:       {}", path.display());
    if !path.exists() {
        println!("Staged:     no");
        println!("Hint:       elastos site stage <dir>");
        return Ok(());
    }
    if !path.join("index.html").exists() {
        println!("Staged:     incomplete (missing index.html)");
        println!("Hint:       elastos site stage <dir>");
        return Ok(());
    }
    println!("Staged:     yes");
    if let Some(head) = load_site_head(MY_WEBSITE_URI)? {
        println!("Active:     yes");
        if let Some(channel_name) = head.payload.channel_name.as_deref() {
            println!("Channel:    {}", channel_name);
        }
        if let Some(release_name) = head.payload.release_name.as_deref() {
            println!("Release:    {}", release_name);
        }
        if let Some(bundle_cid) = head.payload.bundle_cid.as_deref() {
            println!("CID:        {}", bundle_cid);
        }
        println!("Digest:     {}", head.payload.content_digest);
        println!("Signer:     {}", head.signer_did);
        let history = load_site_history(MY_WEBSITE_URI)?;
        println!("History:    {} activations", history.len());
    } else {
        println!("Active:     no");
    }
    Ok(())
}

fn current_unix_millis() -> anyhow::Result<u64> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    u64::try_from(millis).map_err(|_| anyhow::anyhow!("system time overflow"))
}

fn normalize_release_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        anyhow::bail!("release name must not be empty");
    }
    if name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        Ok(name)
    } else {
        anyhow::bail!("release name contains unsupported characters: {}", name);
    }
}

fn normalize_channel_name(name: &str) -> anyhow::Result<String> {
    normalize_release_name(name)
}

async fn publish_site(target: Option<&str>, release: Option<&str>) -> anyhow::Result<()> {
    let (target, target_path) = resolve_site_target(target)?;
    let (bundle_cid, digest) = publish_site_bundle(&target_path).await?;
    let mut published = None;
    if let Some(release) = release {
        let release = normalize_release_name(release)?;
        let record = SiteReleaseRecord {
            schema: SITE_RELEASE_DOMAIN.to_string(),
            target: target.clone(),
            release_name: release.clone(),
            bundle_cid: bundle_cid.clone(),
            content_digest: format!("sha256:{}", digest.digest_hex),
            entry_count: digest.entry_count,
            total_bytes: digest.total_bytes,
            published_at: current_unix_millis()?,
        };
        let release_path = write_site_release(&target, &record)?;
        published = Some((record, release_path));
    }

    println!("Published site");
    println!("  Target: {}", target);
    println!("  CID:    {}", bundle_cid);
    println!("  URI:    elastos://{}", bundle_cid);
    println!("  Digest: sha256:{}", digest.digest_hex);
    if let Some((record, release_path)) = published {
        println!("  Release: {}", record.release_name);
        println!("  State:   {}", release_path.display());
    }
    Ok(())
}

async fn activate_site(
    target: Option<&str>,
    release: Option<&str>,
    channel: Option<&str>,
) -> anyhow::Result<()> {
    let target = resolve_site_target_uri(target)?;
    if release.is_some() && channel.is_some() {
        anyhow::bail!("choose either --release or --channel, not both");
    }

    let payload = if let Some(channel_name) = channel {
        let channel = load_site_channel(&target, channel_name)?;
        let release = load_site_release(&target, &channel.release_name)?;
        SiteHeadPayload {
            schema: SITE_HEAD_DOMAIN.to_string(),
            target: target.clone(),
            bundle_cid: Some(release.bundle_cid.clone()),
            release_name: Some(release.release_name.clone()),
            channel_name: Some(channel.channel_name.clone()),
            content_digest: release.content_digest.clone(),
            entry_count: release.entry_count,
            total_bytes: release.total_bytes,
            activated_at: current_unix_millis()?,
        }
    } else if let Some(release) = release {
        let release = load_site_release(&target, release)?;
        SiteHeadPayload {
            schema: SITE_HEAD_DOMAIN.to_string(),
            target: target.clone(),
            bundle_cid: Some(release.bundle_cid.clone()),
            release_name: Some(release.release_name.clone()),
            channel_name: None,
            content_digest: release.content_digest.clone(),
            entry_count: release.entry_count,
            total_bytes: release.total_bytes,
            activated_at: current_unix_millis()?,
        }
    } else {
        let (_, target_path) = resolve_site_target(Some(&target))?;
        let (bundle_cid, digest) = publish_site_bundle(&target_path).await?;
        SiteHeadPayload {
            schema: SITE_HEAD_DOMAIN.to_string(),
            target: target.clone(),
            bundle_cid: Some(bundle_cid.clone()),
            release_name: None,
            channel_name: None,
            content_digest: format!("sha256:{}", digest.digest_hex),
            entry_count: digest.entry_count,
            total_bytes: digest.total_bytes,
            activated_at: current_unix_millis()?,
        }
    };
    let envelope = write_site_head(&target, payload)?;
    let head_path = edge_site_head_path(&default_data_dir(), &target);

    println!("Activated site");
    println!("  Target: {}", target);
    if let Some(channel_name) = envelope.payload.channel_name.as_deref() {
        println!("  Channel: {}", channel_name);
    }
    if let Some(release_name) = envelope.payload.release_name.as_deref() {
        println!("  Release: {}", release_name);
    }
    if let Some(bundle_cid) = envelope.payload.bundle_cid.as_deref() {
        println!("  CID:    {}", bundle_cid);
    }
    println!("  Digest: {}", envelope.payload.content_digest);
    println!("  Signer: {}", envelope.signer_did);
    println!("  State:  {}", head_path.display());
    Ok(())
}

fn print_site_channels(target: Option<&str>, json: bool) -> anyhow::Result<()> {
    let target = resolve_site_target_uri(target)?;
    let current = load_site_head(&target)?;
    let rows: Vec<SiteReleaseChannelEntry> = load_site_channels(&target)?
        .into_iter()
        .map(|channel| SiteReleaseChannelEntry {
            active: current.as_ref().is_some_and(|head| {
                head.payload.channel_name.as_deref() == Some(channel.channel_name.as_str())
                    && head.payload.release_name.as_deref() == Some(channel.release_name.as_str())
                    && head.payload.bundle_cid.as_deref() == Some(channel.bundle_cid.as_str())
            }),
            channel_name: channel.channel_name,
            release_name: channel.release_name,
            bundle_cid: channel.bundle_cid,
            promoted_at: channel.promoted_at,
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No site release channels for {}", target);
        return Ok(());
    }

    println!("Site release channels");
    println!("  Target: {}", target);
    for row in rows {
        let marker = if row.active { "*" } else { " " };
        println!(
            "{} {}  {:<12}  {:<18}  {}",
            marker, row.promoted_at, row.channel_name, row.release_name, row.bundle_cid
        );
    }
    Ok(())
}

fn print_site_releases(target: Option<&str>, json: bool) -> anyhow::Result<()> {
    let target = resolve_site_target_uri(target)?;
    let current = load_site_head(&target)?;
    let rows: Vec<SiteReleaseEntry> = load_site_releases(&target)?
        .into_iter()
        .map(|release| SiteReleaseEntry {
            active: current.as_ref().is_some_and(|head| {
                head.payload.release_name.as_deref() == Some(release.release_name.as_str())
                    && head.payload.bundle_cid.as_deref() == Some(release.bundle_cid.as_str())
            }),
            release_name: release.release_name,
            bundle_cid: release.bundle_cid,
            content_digest: release.content_digest,
            published_at: release.published_at,
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No named site releases for {}", target);
        return Ok(());
    }

    println!("Named site releases");
    println!("  Target: {}", target);
    for row in rows {
        let marker = if row.active { "*" } else { " " };
        println!(
            "{} {}  {}  {}",
            marker, row.published_at, row.release_name, row.bundle_cid
        );
    }
    Ok(())
}

fn print_site_history(target: Option<&str>, json: bool) -> anyhow::Result<()> {
    let target = resolve_site_target_uri(target)?;
    let current = load_site_head(&target)?;
    let history = load_site_history(&target)?;
    let rows: Vec<SiteHeadHistoryEntry> = history
        .iter()
        .map(|entry| SiteHeadHistoryEntry {
            release_name: entry.payload.release_name.clone(),
            channel_name: entry.payload.channel_name.clone(),
            bundle_cid: entry.payload.bundle_cid.clone(),
            content_digest: entry.payload.content_digest.clone(),
            activated_at: entry.payload.activated_at,
            signer_did: entry.signer_did.clone(),
            current: current.as_ref().is_some_and(|head| {
                head.signature == entry.signature
                    && head.payload.activated_at == entry.payload.activated_at
            }),
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No site activation history for {}", target);
        return Ok(());
    }

    println!("Site activation history");
    println!("  Target: {}", target);
    for row in rows {
        let marker = if row.current { "*" } else { " " };
        println!(
            "{} {}  {:<12}  {:<18}  {}  {}",
            marker,
            row.activated_at,
            row.channel_name.as_deref().unwrap_or("-"),
            row.release_name.as_deref().unwrap_or("-"),
            row.bundle_cid.as_deref().unwrap_or("-"),
            row.signer_did
        );
    }
    Ok(())
}

fn rollback_site(revision: Option<&str>, target: Option<&str>) -> anyhow::Result<()> {
    let target = resolve_site_target_uri(target)?;
    let current = load_site_head(&target)?
        .ok_or_else(|| anyhow::anyhow!("no active site head for {}", target))?;
    let history = load_site_history(&target)?;
    let selected = select_history_entry_for_rollback(&history, &current, revision)?;

    let payload = SiteHeadPayload {
        schema: SITE_HEAD_DOMAIN.to_string(),
        target: target.to_string(),
        bundle_cid: selected.payload.bundle_cid.clone(),
        release_name: selected.payload.release_name.clone(),
        channel_name: selected.payload.channel_name.clone(),
        content_digest: selected.payload.content_digest.clone(),
        entry_count: selected.payload.entry_count,
        total_bytes: selected.payload.total_bytes,
        activated_at: current_unix_millis()?,
    };
    let envelope = write_site_head(&target, payload)?;
    println!("Rolled back site");
    println!("  Target: {}", target);
    if let Some(channel_name) = envelope.payload.channel_name.as_deref() {
        println!("  Channel: {}", channel_name);
    }
    if let Some(release_name) = envelope.payload.release_name.as_deref() {
        println!("  Release: {}", release_name);
    }
    if let Some(bundle_cid) = envelope.payload.bundle_cid.as_deref() {
        println!("  CID:    {}", bundle_cid);
    }
    println!("  Digest: {}", envelope.payload.content_digest);
    println!("  Signer: {}", envelope.signer_did);
    Ok(())
}

fn resolve_site_target(target: Option<&str>) -> anyhow::Result<(String, PathBuf)> {
    let target = resolve_site_target_uri(target)?;
    let target_path = rooted_localhost_fs_path(&default_data_dir(), &target).ok_or_else(|| {
        anyhow::anyhow!(
            "site target must be a rooted file-backed localhost path, got: {}",
            target
        )
    })?;
    validate_site_path(&target_path)?;
    Ok((target, target_path))
}

fn resolve_site_target_uri(target: Option<&str>) -> anyhow::Result<String> {
    let target = target.unwrap_or(MY_WEBSITE_URI).to_string();
    rooted_localhost_fs_path(&default_data_dir(), &target).ok_or_else(|| {
        anyhow::anyhow!(
            "site target must be a rooted file-backed localhost path, got: {}",
            target
        )
    })?;
    Ok(target)
}

fn bind_domain(domain: &str, target: Option<&str>) -> anyhow::Result<()> {
    let domain = normalize_domain(domain)?;
    let target = target.unwrap_or(MY_WEBSITE_URI);
    let target_path = rooted_localhost_fs_path(&default_data_dir(), target).ok_or_else(|| {
        anyhow::anyhow!(
            "domain target must be a rooted file-backed localhost path, got: {}",
            target
        )
    })?;
    validate_site_path(&target_path)?;

    let binding_path = edge_binding_path(&default_data_dir(), &domain);
    if let Some(parent) = binding_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let binding = SiteDomainBinding {
        domain: domain.clone(),
        target: target.to_string(),
    };
    fs::write(&binding_path, serde_json::to_vec_pretty(&binding)?)?;

    println!("Bound public domain");
    println!("  Domain: {}", domain);
    println!("  Target: {}", target);
    println!("  State:  {}", binding_path.display());
    Ok(())
}

fn promote_site_release(channel: &str, release: &str, target: Option<&str>) -> anyhow::Result<()> {
    let target = resolve_site_target_uri(target)?;
    let channel_name = normalize_channel_name(channel)?;
    let release = load_site_release(&target, release)?;
    let record = SiteReleaseChannelRecord {
        schema: SITE_CHANNEL_DOMAIN.to_string(),
        target: target.clone(),
        channel_name: channel_name.clone(),
        release_name: release.release_name.clone(),
        bundle_cid: release.bundle_cid.clone(),
        promoted_at: current_unix_millis()?,
    };
    let channel_path = write_site_channel(&target, &record)?;

    println!("Promoted site release");
    println!("  Target:  {}", target);
    println!("  Channel: {}", record.channel_name);
    println!("  Release: {}", record.release_name);
    println!("  CID:     {}", record.bundle_cid);
    println!("  State:   {}", channel_path.display());
    Ok(())
}

fn load_site_head(target: &str) -> anyhow::Result<Option<SiteHeadEnvelope>> {
    let head_path = edge_site_head_path(&default_data_dir(), target);
    if !head_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&head_path)?;
    let head: SiteHeadEnvelope = serde_json::from_slice(&bytes)?;
    Ok(Some(head))
}

fn write_site_release(target: &str, record: &SiteReleaseRecord) -> anyhow::Result<PathBuf> {
    let release_path =
        publisher_site_release_path(&default_data_dir(), target, &record.release_name);
    if let Some(parent) = release_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&release_path, serde_json::to_vec_pretty(record)?)?;
    Ok(release_path)
}

fn load_site_release(target: &str, release_name: &str) -> anyhow::Result<SiteReleaseRecord> {
    let release_name = normalize_release_name(release_name)?;
    let release_path = publisher_site_release_path(&default_data_dir(), target, &release_name);
    let bytes = fs::read(&release_path).with_context(|| {
        format!(
            "named site release '{}' not found for {}. Publish it first with: elastos site publish --release {}",
            release_name, target, release_name
        )
    })?;
    let release: SiteReleaseRecord = serde_json::from_slice(&bytes)?;
    Ok(release)
}

fn load_site_releases(target: &str) -> anyhow::Result<Vec<SiteReleaseRecord>> {
    let releases_dir = publisher_site_releases_dir(&default_data_dir(), target);
    if !releases_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&releases_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        let release: SiteReleaseRecord = serde_json::from_slice(&bytes)?;
        entries.push(release);
    }
    entries.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    Ok(entries)
}

fn write_site_channel(target: &str, record: &SiteReleaseChannelRecord) -> anyhow::Result<PathBuf> {
    let channel_path = edge_release_channel_path(&default_data_dir(), target, &record.channel_name);
    if let Some(parent) = channel_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&channel_path, serde_json::to_vec_pretty(record)?)?;
    Ok(channel_path)
}

fn load_site_channel(target: &str, channel_name: &str) -> anyhow::Result<SiteReleaseChannelRecord> {
    let channel_name = normalize_channel_name(channel_name)?;
    let channel_path = edge_release_channel_path(&default_data_dir(), target, &channel_name);
    let bytes = fs::read(&channel_path).with_context(|| {
        format!(
            "site release channel '{}' not found for {}. Promote one first with: elastos site promote {} <release>",
            channel_name, target, channel_name
        )
    })?;
    let channel: SiteReleaseChannelRecord = serde_json::from_slice(&bytes)?;
    Ok(channel)
}

fn load_site_channels(target: &str) -> anyhow::Result<Vec<SiteReleaseChannelRecord>> {
    let channels_dir = edge_release_channels_dir(&default_data_dir(), target);
    if !channels_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&channels_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        let channel: SiteReleaseChannelRecord = serde_json::from_slice(&bytes)?;
        entries.push(channel);
    }
    entries.sort_by(|a, b| b.promoted_at.cmp(&a.promoted_at));
    Ok(entries)
}

fn load_site_history(target: &str) -> anyhow::Result<Vec<SiteHeadEnvelope>> {
    let history_dir = edge_site_history_dir(&default_data_dir(), target);
    if !history_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&history_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        let head: SiteHeadEnvelope = serde_json::from_slice(&bytes)?;
        entries.push(head);
    }
    entries.sort_by(|a, b| b.payload.activated_at.cmp(&a.payload.activated_at));
    Ok(entries)
}

fn write_site_head(target: &str, payload: SiteHeadPayload) -> anyhow::Result<SiteHeadEnvelope> {
    let canonical = serde_json::to_string(&payload)?;
    let signing_key = load_or_create_share_key()?;
    let (signature, signer_did) =
        domain_separated_sign(&signing_key, SITE_HEAD_DOMAIN, canonical.as_bytes());
    let envelope = SiteHeadEnvelope {
        payload,
        signature,
        signer_did,
    };

    let head_path = edge_site_head_path(&default_data_dir(), target);
    if let Some(parent) = head_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&head_path, serde_json::to_vec_pretty(&envelope)?)?;

    let history_dir = edge_site_history_dir(&default_data_dir(), target);
    fs::create_dir_all(&history_dir)?;
    let history_name = format!(
        "{:020}-{}.json",
        envelope.payload.activated_at,
        site_history_key(&envelope)
    );
    fs::write(
        history_dir.join(history_name),
        serde_json::to_vec_pretty(&envelope)?,
    )?;

    Ok(envelope)
}

fn site_history_key(head: &SiteHeadEnvelope) -> String {
    let label = head
        .payload
        .channel_name
        .as_deref()
        .or(head.payload.release_name.as_deref())
        .or(head.payload.bundle_cid.as_deref())
        .unwrap_or(&head.payload.content_digest);
    let mut key = sanitize_edge_state_name(label)
        .chars()
        .take(16)
        .collect::<String>();
    let suffix = sanitize_edge_state_name(&head.signature)
        .chars()
        .take(12)
        .collect::<String>();
    if !suffix.is_empty() {
        key.push('-');
        key.push_str(&suffix);
    }
    if !key.is_empty() {
        return key;
    }
    let fallback = head
        .payload
        .bundle_cid
        .as_deref()
        .unwrap_or(&head.payload.content_digest);
    sanitize_edge_state_name(fallback)
        .chars()
        .take(24)
        .collect()
}

fn select_history_entry_for_rollback<'a>(
    history: &'a [SiteHeadEnvelope],
    current: &SiteHeadEnvelope,
    revision: Option<&str>,
) -> anyhow::Result<&'a SiteHeadEnvelope> {
    if let Some(revision) = revision {
        let revision = revision.trim();
        let matches_revision = |entry: &SiteHeadEnvelope| {
            entry.payload.channel_name.as_deref() == Some(revision)
                || entry
                    .payload
                    .channel_name
                    .as_deref()
                    .is_some_and(|name| name.starts_with(revision))
                || entry.payload.release_name.as_deref() == Some(revision)
                || entry
                    .payload
                    .release_name
                    .as_deref()
                    .is_some_and(|name| name.starts_with(revision))
                || entry
                    .payload
                    .bundle_cid
                    .as_deref()
                    .is_some_and(|cid| cid == revision || cid.starts_with(revision))
        };
        let is_current = |entry: &SiteHeadEnvelope| {
            entry.signature == current.signature
                && entry.payload.activated_at == current.payload.activated_at
        };

        if let Some(selected) = history
            .iter()
            .find(|entry| matches_revision(entry) && !is_current(entry))
        {
            return Ok(selected);
        }
        if history.iter().any(matches_revision) {
            anyhow::bail!("selected site revision is already active: {}", revision);
        }
        anyhow::bail!("no historical site head matches {}", revision);
    }

    history
        .iter()
        .find(|entry| {
            entry.signature != current.signature
                || entry.payload.activated_at != current.payload.activated_at
        })
        .ok_or_else(|| anyhow::anyhow!("no previous site activation available"))
}

fn compute_site_digest(entries: &[(String, Vec<u8>)]) -> SiteDigest {
    let mut manifest_hasher = sha2::Sha256::new();
    let mut total_bytes = 0u64;
    for (rel_path, bytes) in entries {
        let file_digest = sha2::Sha256::digest(bytes);
        manifest_hasher.update(rel_path.as_bytes());
        manifest_hasher.update(b"\0");
        manifest_hasher.update(file_digest);
        manifest_hasher.update(b"\0");
        total_bytes += bytes.len() as u64;
    }

    SiteDigest {
        digest_hex: hex::encode(manifest_hasher.finalize()),
        entry_count: entries.len() as u64,
        total_bytes,
    }
}

fn collect_site_files(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(String, Vec<u8>)>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            anyhow::bail!(
                "site activation does not allow symlinks: {}",
                path.display()
            );
        }
        if file_type.is_dir() {
            collect_site_files(root, &path, entries)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .ok()
                .and_then(|p| p.to_str())
                .ok_or_else(|| anyhow::anyhow!("invalid site path: {}", path.display()))?
                .replace('\\', "/");
            entries.push((rel, fs::read(&path)?));
        }
    }
    Ok(())
}

fn collect_site_entries(root: &Path) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut entries = Vec::new();
    collect_site_files(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

fn materialize_site_bundle(entries: &[(String, Vec<u8>)]) -> anyhow::Result<tempfile::TempDir> {
    let bundle_dir = tempfile::Builder::new().prefix("elastos-site-").tempdir()?;
    for (rel_path, bytes) in entries {
        let dest = bundle_dir.path().join(rel_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(dest, bytes)?;
    }
    Ok(bundle_dir)
}

async fn publish_site_bundle(target_path: &Path) -> anyhow::Result<(String, SiteDigest)> {
    let entries = collect_site_entries(target_path)?;
    let digest = compute_site_digest(&entries);
    let bundle_dir = materialize_site_bundle(&entries)?;
    let ipfs = get_ipfs_bridge().await?;
    let bundle_cid = ipfs.add_directory_from_path(bundle_dir.path()).await?;
    Ok((bundle_cid, digest))
}

async fn get_ipfs_bridge() -> anyhow::Result<IpfsBridge> {
    let binary = crate::resolve_verified_provider_binary(
        "ipfs-provider",
        "ipfs-provider not found. Run:\n\n  elastos setup --with kubo --with ipfs-provider",
    )?;
    let bridge = ProviderBridge::spawn(&binary, BridgeProviderConfig::default())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn ipfs-provider: {}", e))?;
    Ok(IpfsBridge::new(std::sync::Arc::new(bridge)))
}

fn normalize_domain(domain: &str) -> anyhow::Result<String> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        anyhow::bail!("domain must not be empty");
    }
    if domain
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
    {
        Ok(domain)
    } else {
        anyhow::bail!("domain contains unsupported characters: {}", domain);
    }
}

async fn serve_site_impl(
    mode: &str,
    addr: &str,
    domain: Option<&str>,
    browser: bool,
    public_timeout: u64,
    open_subpath: Option<&str>,
) -> anyhow::Result<()> {
    let site_path = site_root_dir();
    validate_site_path(&site_path)?;

    let requested_addr = if addr.trim().is_empty() {
        DEFAULT_SITE_ADDR
    } else {
        addr
    };
    let site = get_site_bridge(&site_path).await?;
    let status = site.start(requested_addr).await?;
    let base_local_url = status
        .local_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("site-provider start response missing local_url"))?;
    let local_url = format!(
        "{}{}",
        base_local_url.trim_end_matches('/'),
        suffix_path(open_subpath)?
    );

    println!("Serving MyWebSite");
    println!("  Local root:   {}", MY_WEBSITE_URI);
    println!("  Path:         {}", site_path.display());
    println!("  Gateway mode: {}", mode);
    println!("  Local URL:    {}", local_url);

    if mode == "local" {
        if let Some(domain) = domain {
            println!("  Domain hint:  {}", format_domain_hint(domain));
        }
        if browser {
            crate::open_browser(&local_url);
        }
        println!();
        println!("Site is live. Press Ctrl+C to stop.");
        wait_for_stop_signal().await?;
        let _ = site.shutdown().await;
        return Ok(());
    }

    if mode != "ephemeral" {
        let _ = site.shutdown().await;
        anyhow::bail!(
            "unsupported site mode '{}'. Expected: local, ephemeral",
            mode
        );
    }

    let target = base_local_url.trim_end_matches('/').to_string();
    let tunnel = get_tunnel_bridge().await?;
    let public_url = start_public_tunnel(&tunnel, &target, public_timeout).await?;
    let public_url = format!("{}{}", public_url, suffix_path(open_subpath)?);
    println!("  Public URL:   {}", public_url);

    if browser {
        crate::open_browser(&public_url);
    }

    println!();
    println!("Site is live. Press Ctrl+C to stop.");
    wait_for_stop_signal().await?;
    let _ = tunnel.shutdown().await;
    let _ = site.shutdown().await;
    Ok(())
}

fn suffix_path(subpath: Option<&str>) -> anyhow::Result<String> {
    let Some(path) = subpath else {
        return Ok("/".to_string());
    };
    if path.is_empty() {
        return Ok("/".to_string());
    }

    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => anyhow::bail!("invalid local site path '{}'", path),
        }
    }

    Ok(format!("/{}/", path.trim_matches('/')))
}

fn format_domain_hint(domain: &str) -> String {
    if domain.starts_with("http://") || domain.starts_with("https://") {
        domain.to_string()
    } else {
        format!("http://{}", domain)
    }
}

fn get_tunnel_binary() -> anyhow::Result<PathBuf> {
    crate::resolve_verified_provider_binary(
        "tunnel-provider",
        "tunnel-provider not found. Run:\n\n  elastos setup --with cloudflared,tunnel-provider",
    )
}

fn get_site_binary() -> anyhow::Result<PathBuf> {
    crate::resolve_verified_provider_binary(
        "site-provider",
        "site-provider not found. Run:\n\n  elastos setup --with site-provider",
    )
}

async fn get_site_bridge(site_path: &Path) -> anyhow::Result<SiteBridge> {
    let binary = get_site_binary()?;
    let config = BridgeProviderConfig {
        base_path: site_path.display().to_string(),
        extra: serde_json::json!({
            "site_root": site_path.display().to_string(),
        }),
        ..BridgeProviderConfig::default()
    };
    let bridge = ProviderBridge::spawn(&binary, config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn site-provider: {}", e))?;
    Ok(SiteBridge { bridge })
}

async fn get_tunnel_bridge() -> anyhow::Result<TunnelBridge> {
    let binary = get_tunnel_binary()?;
    let bridge = ProviderBridge::spawn(&binary, BridgeProviderConfig::default())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn tunnel-provider: {}", e))?;
    Ok(TunnelBridge { bridge })
}

async fn start_public_tunnel(
    tunnel: &TunnelBridge,
    target: &str,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let start_status = match tunnel.start(target).await {
        Ok(status) => status,
        Err(err) => {
            let _ = tunnel.shutdown().await;
            return Err(err);
        }
    };

    if let Some(url) = start_status.url.as_deref() {
        return Ok(url.trim_end_matches('/').to_string());
    }
    if !start_status.running {
        let detail = tunnel_status_detail(&start_status);
        let _ = tunnel.shutdown().await;
        anyhow::bail!(
            "tunnel-provider exited before publishing a public URL.{}",
            detail
        );
    }

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if Instant::now() >= deadline {
            let last = tunnel.status().await.unwrap_or_default();
            let detail = tunnel_status_detail(&last);
            let _ = tunnel.shutdown().await;
            anyhow::bail!(
                "timed out after {}s waiting for a public URL.{}",
                timeout_secs,
                detail
            );
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        let status = match tunnel.status().await {
            Ok(status) => status,
            Err(err) => {
                let _ = tunnel.shutdown().await;
                return Err(err);
            }
        };

        if let Some(url) = status.url.as_deref() {
            return Ok(url.trim_end_matches('/').to_string());
        }
        if !status.running {
            let detail = tunnel_status_detail(&status);
            let _ = tunnel.shutdown().await;
            anyhow::bail!(
                "tunnel-provider exited before publishing a public URL.{}",
                detail
            );
        }
    }
}

fn parse_tunnel_status_response(resp: serde_json::Value, op: &str) -> anyhow::Result<TunnelStatus> {
    if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
        if status == "error" {
            let code = resp
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            let message = resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("tunnel-provider {} failed [{}]: {}", op, code, message);
        }
    }

    let data = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(data)
        .map_err(|e| anyhow::anyhow!("Invalid tunnel-provider {} response: {}", op, e))
}

fn parse_site_status_response(resp: serde_json::Value, op: &str) -> anyhow::Result<SiteStatus> {
    if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
        if status == "error" {
            let code = resp
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            let message = resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            anyhow::bail!("site-provider {} failed [{}]: {}", op, code, message);
        }
    }

    let data = resp.get("data").cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(data)
        .map_err(|e| anyhow::anyhow!("Invalid site-provider {} response: {}", op, e))
}

fn tunnel_status_detail(status: &TunnelStatus) -> String {
    status
        .last_log
        .as_deref()
        .map(|log| format!(" Last status: {}", log))
        .unwrap_or_default()
}

async fn wait_for_stop_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm = signal(SignalKind::terminate())
            .map_err(|e| anyhow::anyhow!("failed to install SIGTERM handler: {}", e))?;

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result?;
            }
            _ = sigterm.recv() => {}
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok(())
    }
}

fn atomic_copy_dir(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow::anyhow!("destination has no parent: {}", dest.display()))?;
    fs::create_dir_all(parent)?;

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let tmp = parent.join(format!(
        ".{}.site-tmp-{}-{}",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("site"),
        std::process::id(),
        millis
    ));

    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    copy_dir_recursive(src, &tmp)?;
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            anyhow::bail!("site staging does not allow symlinks: {}", path.display());
        }
        if file_type.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if file_type.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target)
                .with_context(|| format!("copy {} -> {}", path.display(), target.display()))?;
        } else {
            anyhow::bail!("unsupported site entry type: {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_public_site_uri() {
        let parsed = parse_public_site_uri("localhost://MyWebSite/docs");
        assert_eq!(parsed, Some("docs".to_string()));
        assert_eq!(
            parse_public_site_uri("localhost://MyWebSite"),
            Some(String::new())
        );
        assert_eq!(parse_public_site_uri("elastos://Qm123"), None);
    }

    #[test]
    fn test_suffix_path_validation() {
        assert_eq!(suffix_path(None).unwrap(), "/");
        assert_eq!(suffix_path(Some("")).unwrap(), "/");
        assert_eq!(suffix_path(Some("docs")).unwrap(), "/docs/");
        assert!(suffix_path(Some("../etc")).is_err());
    }

    #[test]
    fn test_normalize_domain() {
        assert_eq!(
            normalize_domain("Elastos.ElacityLabs.com.").unwrap(),
            "elastos.elacitylabs.com"
        );
        assert!(normalize_domain("bad host").is_err());
    }

    #[test]
    fn test_normalize_release_name() {
        assert_eq!(
            normalize_release_name("Weekend-Demo").unwrap(),
            "weekend-demo"
        );
        assert!(normalize_release_name("bad release").is_err());
    }

    #[test]
    fn test_normalize_channel_name() {
        assert_eq!(normalize_channel_name("Live").unwrap(), "live");
        assert!(normalize_channel_name("bad channel").is_err());
    }

    #[test]
    fn test_compute_site_digest_is_stable() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("index.html"), "<html>ok</html>").unwrap();
        fs::create_dir_all(dir.path().join("assets")).unwrap();
        fs::write(
            dir.path().join("assets").join("app.js"),
            "console.log('ok');",
        )
        .unwrap();

        let entries_a = collect_site_entries(dir.path()).unwrap();
        let entries_b = collect_site_entries(dir.path()).unwrap();
        let digest_a = compute_site_digest(&entries_a);
        let digest_b = compute_site_digest(&entries_b);

        assert_eq!(digest_a.digest_hex, digest_b.digest_hex);
        assert_eq!(digest_a.entry_count, 2);
        assert!(digest_a.total_bytes > 0);
    }

    #[test]
    fn test_select_history_entry_for_rollback_prefers_previous() {
        let current = SiteHeadEnvelope {
            payload: SiteHeadPayload {
                schema: SITE_HEAD_DOMAIN.to_string(),
                target: MY_WEBSITE_URI.to_string(),
                bundle_cid: Some("bafy-current".to_string()),
                release_name: Some("live".to_string()),
                channel_name: Some("live".to_string()),
                content_digest: "sha256:current".to_string(),
                entry_count: 1,
                total_bytes: 10,
                activated_at: 20,
            },
            signature: "sig-current".to_string(),
            signer_did: "did:key:current".to_string(),
        };
        let previous = SiteHeadEnvelope {
            payload: SiteHeadPayload {
                schema: SITE_HEAD_DOMAIN.to_string(),
                target: MY_WEBSITE_URI.to_string(),
                bundle_cid: Some("bafy-prev".to_string()),
                release_name: Some("v1".to_string()),
                channel_name: Some("live".to_string()),
                content_digest: "sha256:prev".to_string(),
                entry_count: 1,
                total_bytes: 10,
                activated_at: 10,
            },
            signature: "sig-prev".to_string(),
            signer_did: "did:key:prev".to_string(),
        };
        let history = vec![current.clone(), previous.clone()];

        let selected = select_history_entry_for_rollback(&history, &current, None).unwrap();
        assert_eq!(selected.payload.bundle_cid.as_deref(), Some("bafy-prev"));

        let selected =
            select_history_entry_for_rollback(&history, &current, Some("bafy-prev")).unwrap();
        assert_eq!(selected.signature, "sig-prev");

        let selected = select_history_entry_for_rollback(&history, &current, Some("v1")).unwrap();
        assert_eq!(selected.payload.bundle_cid.as_deref(), Some("bafy-prev"));
    }
}
