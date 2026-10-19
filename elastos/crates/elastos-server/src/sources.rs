//! Trusted release sources: configuration, persistence, and CLI handlers.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use elastos_common::localhost::installation_release_head_path;

use crate::ownership;

const ALLOWED_RELEASE_CHANNELS: &[&str] = &["stable", "canary", "jetson-test"];

/// Default data directory: `$XDG_DATA_HOME/elastos` or `$HOME/.local/share/elastos`.
pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("elastos")
}

pub struct OwnershipRepairGuard {
    path: PathBuf,
}

impl OwnershipRepairGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for OwnershipRepairGuard {
    fn drop(&mut self) {
        let _ = ownership::repair_path_recursive(&self.path);
    }
}

pub fn trusted_sources_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("sources.json")
}

pub fn default_install_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".local/bin/elastos")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrustedSource {
    pub name: String,
    #[serde(default)]
    pub publisher_dids: Vec<String>,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub discovery_uri: String,
    #[serde(default)]
    pub connect_ticket: String,
    #[serde(default)]
    pub gateways: Vec<String>,
    #[serde(default)]
    pub install_path: String,
    #[serde(default)]
    pub installed_version: String,
    #[serde(default)]
    pub head_cid: String,
    /// Stable raw Iroh endpoint ID string for durable P2P transport connections.
    #[serde(default)]
    pub publisher_node_id: String,
    /// IPNS name for mutable release head pointer (Kubo peer ID or key name).
    #[serde(default)]
    pub ipns_name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrustedSourcesConfig {
    pub schema: String,
    #[serde(default)]
    pub default_source: String,
    #[serde(default)]
    pub sources: Vec<TrustedSource>,
}

impl TrustedSourcesConfig {
    pub fn empty() -> Self {
        Self {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: String::new(),
            sources: Vec::new(),
        }
    }

    pub fn default_source(&self) -> Option<&TrustedSource> {
        if !self.default_source.is_empty() {
            self.sources.iter().find(|s| s.name == self.default_source)
        } else {
            self.sources.first()
        }
    }

    pub fn source_named(&self, name: Option<&str>) -> Option<&TrustedSource> {
        match name {
            Some(name) => self.sources.iter().find(|s| s.name == name),
            None => self.default_source(),
        }
    }

    pub fn source_named_mut(&mut self, name: Option<&str>) -> Option<&mut TrustedSource> {
        let name = match name {
            Some(name) => name.to_string(),
            None if !self.default_source.is_empty() => self.default_source.clone(),
            None => self.sources.first().map(|s| s.name.clone())?,
        };
        self.sources.iter_mut().find(|s| s.name == name)
    }

    pub fn upsert_source(&mut self, source: TrustedSource) {
        if let Some(existing) = self.sources.iter_mut().find(|s| s.name == source.name) {
            *existing = source;
        } else {
            self.sources.push(source);
        }
        if self.default_source.is_empty() {
            self.default_source = self.sources[0].name.clone();
        }
    }
}

pub fn infer_install_path() -> PathBuf {
    let default_path = default_install_path();
    if default_path.is_file() {
        default_path
    } else {
        std::env::current_exe().unwrap_or(default_path)
    }
}

pub fn save_trusted_sources(
    data_dir: &std::path::Path,
    config: &TrustedSourcesConfig,
) -> anyhow::Result<()> {
    let _writer = crate::install_transaction::acquire_installed_writer(data_dir)?;
    std::fs::create_dir_all(data_dir)?;
    let json = serde_json::to_string_pretty(config)?;
    std::fs::write(trusted_sources_path(data_dir), json)?;
    Ok(())
}

pub fn load_trusted_sources(data_dir: &std::path::Path) -> anyhow::Result<TrustedSourcesConfig> {
    let path = trusted_sources_path(data_dir);
    if path.exists() {
        let data = std::fs::read_to_string(&path)?;
        let mut config: TrustedSourcesConfig = serde_json::from_str(&data)?;
        if config.schema.is_empty() {
            config.schema = "elastos.trusted-sources/v1".to_string();
        }
        if config.default_source.is_empty() && !config.sources.is_empty() {
            config.default_source = config.sources[0].name.clone();
        }
        return Ok(config);
    }

    Ok(TrustedSourcesConfig::empty())
}

pub fn normalize_gateways(gateways: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for gateway in gateways {
        let trimmed = gateway.trim().trim_end_matches('/').to_string();
        if !trimmed.is_empty() && !normalized.iter().any(|g| g == &trimmed) {
            normalized.push(trimmed);
        }
    }
    normalized
}

fn validate_release_channel(channel: &str) -> anyhow::Result<()> {
    if ALLOWED_RELEASE_CHANNELS.contains(&channel) {
        Ok(())
    } else {
        anyhow::bail!(
            "Unsupported release channel '{}'. Allowed channels: {}",
            channel,
            ALLOWED_RELEASE_CHANNELS.join(", ")
        );
    }
}

pub fn local_session_owner(data_dir: &std::path::Path) -> anyhow::Result<String> {
    let (_signing_key, did) = elastos_identity::load_or_create_did(data_dir)?;
    Ok(did)
}

/// Run a trusted-source subcommand.
///
/// `source_discovery_uri_fn` resolves the discovery URI from (publisher_did, channel).
pub fn run_source_command(
    cmd: SourceCommand,
    source_discovery_uri_fn: fn(&str, &str) -> String,
) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    run_source_command_with_confirmation(
        cmd,
        source_discovery_uri_fn,
        &data_dir,
        confirm_signer_change,
    )
}

fn confirm_signer_change(
    previous: &TrustedSource,
    replacement: &TrustedSource,
) -> anyhow::Result<bool> {
    confirm_signer_change_with_io(
        previous,
        replacement,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
    )
}

fn confirm_signer_change_with_io(
    previous: &TrustedSource,
    replacement: &TrustedSource,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> anyhow::Result<bool> {
    let publisher = &replacement.publisher_dids[0];
    writeln!(
        output,
        "Replace the release signer for source '{}'.",
        previous.name
    )?;
    writeln!(
        output,
        "Current signer: {}",
        previous.publisher_dids.join(", ")
    )?;
    writeln!(output, "New signer: {}", publisher)?;
    writeln!(
        output,
        "Compare the new DID with the publisher's published DID."
    )?;
    write!(
        output,
        "Type the complete new DID to confirm, or press Enter to cancel: "
    )?;
    output.flush()?;
    let mut response = String::new();
    input.read_line(&mut response)?;
    Ok(response.trim() == publisher)
}

fn run_source_command_with_confirmation(
    cmd: SourceCommand,
    source_discovery_uri_fn: fn(&str, &str) -> String,
    data_dir: &Path,
    confirm: impl FnOnce(&TrustedSource, &TrustedSource) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let mut config = load_trusted_sources(data_dir)?;

    match cmd {
        SourceCommand::FetchFile { .. } => {
            anyhow::bail!("Carrier file fetch requires the asynchronous source handler")
        }
        SourceCommand::Add {
            name,
            publisher,
            channel,
            discovery_uri,
            connect_ticket,
            gateways,
            install_path,
            head_cid,
            publisher_node_id,
            ipns_name,
        } => {
            crate::crypto::decode_did_key(&publisher)?;
            let previous = config.source_named(Some(&name));
            if let Some(channel) = &channel {
                validate_release_channel(channel)?;
            }
            let channel = channel
                .or_else(|| previous.map(|source| source.channel.clone()))
                .unwrap_or_else(|| "stable".to_string());
            validate_release_channel(if channel.is_empty() {
                "stable"
            } else {
                &channel
            })?;
            let resolved_discovery_uri = discovery_uri
                .filter(|uri| !uri.trim().is_empty())
                .or_else(|| {
                    previous
                        .filter(|source| {
                            !source.discovery_uri.is_empty()
                                && source.discovery_uri
                                    != source_discovery_uri_fn(
                                        source
                                            .publisher_dids
                                            .first()
                                            .map(String::as_str)
                                            .unwrap_or(""),
                                        &source.channel,
                                    )
                        })
                        .map(|source| source.discovery_uri.clone())
                })
                .unwrap_or_else(|| source_discovery_uri_fn(&publisher, &channel));
            let source = TrustedSource {
                name: name.clone(),
                publisher_dids: vec![publisher],
                channel,
                discovery_uri: resolved_discovery_uri,
                connect_ticket: connect_ticket
                    .or_else(|| previous.map(|source| source.connect_ticket.clone()))
                    .unwrap_or_default(),
                gateways: if gateways.is_empty() {
                    previous
                        .map(|source| source.gateways.clone())
                        .unwrap_or_default()
                } else {
                    normalize_gateways(&gateways)
                },
                install_path: install_path
                    .map(|path| path.display().to_string())
                    .or_else(|| previous.map(|source| source.install_path.clone()))
                    .unwrap_or_else(|| infer_install_path().display().to_string()),
                installed_version: previous
                    .map(|s| s.installed_version.clone())
                    .unwrap_or_default(),
                head_cid: head_cid
                    .unwrap_or_else(|| previous.map(|s| s.head_cid.clone()).unwrap_or_default()),
                publisher_node_id: publisher_node_id.unwrap_or_else(|| {
                    previous
                        .map(|s| s.publisher_node_id.clone())
                        .unwrap_or_default()
                }),
                ipns_name: ipns_name
                    .unwrap_or_else(|| previous.map(|s| s.ipns_name.clone()).unwrap_or_default()),
            };
            if let Some(previous) = previous {
                if previous.publisher_dids != source.publisher_dids && !confirm(previous, &source)?
                {
                    anyhow::bail!(
                        "Release signer change cancelled; trusted source settings are unchanged"
                    );
                }
            }
            config.upsert_source(source);
            if config.default_source.is_empty() {
                config.default_source = name.clone();
            }
            save_trusted_sources(data_dir, &config)?;

            println!("Trusted source '{}' saved.", name);
        }
        SourceCommand::List => {
            if config.sources.is_empty() {
                println!("No trusted sources configured.");
            } else {
                println!("Trusted sources:");
                for source in &config.sources {
                    let marker = if config.default_source == source.name {
                        " [default]"
                    } else {
                        ""
                    };
                    let publisher = source.publisher_dids.first().cloned().unwrap_or_default();
                    println!(
                        "  - {}{}  publisher={}  channel={}  version={}",
                        source.name,
                        marker,
                        publisher,
                        if source.channel.is_empty() {
                            "stable"
                        } else {
                            &source.channel
                        },
                        if source.installed_version.is_empty() {
                            "unknown"
                        } else {
                            &source.installed_version
                        }
                    );
                }
            }
        }
        SourceCommand::Show { name } => {
            let source = config
                .source_named(name.as_deref())
                .ok_or_else(|| anyhow::anyhow!("Trusted source not found"))?;
            println!("Source:    {}", source.name);
            println!(
                "Publisher: {}",
                source.publisher_dids.first().cloned().unwrap_or_default()
            );
            println!(
                "Channel:   {}",
                if source.channel.is_empty() {
                    "stable"
                } else {
                    &source.channel
                }
            );
            println!(
                "Version:   {}",
                if source.installed_version.is_empty() {
                    "unknown"
                } else {
                    &source.installed_version
                }
            );
            println!(
                "Discovery: {}",
                if source.discovery_uri.is_empty() {
                    "unknown"
                } else {
                    &source.discovery_uri
                }
            );
            println!(
                "Bootstrap: {}",
                if source.connect_ticket.is_empty() {
                    "none"
                } else {
                    "peer ticket configured"
                }
            );
            println!(
                "Head CID:  {}",
                if source.head_cid.is_empty() {
                    "unknown"
                } else {
                    &source.head_cid
                }
            );
            println!(
                "Install:   {}",
                if source.install_path.is_empty() {
                    "unknown"
                } else {
                    &source.install_path
                }
            );
            println!(
                "Node ID:   {}",
                if source.publisher_node_id.is_empty() {
                    "none"
                } else {
                    &source.publisher_node_id
                }
            );
            println!(
                "IPNS:      {}",
                if source.ipns_name.is_empty() {
                    "none"
                } else {
                    &source.ipns_name
                }
            );
            if source.gateways.is_empty() {
                println!("Gateways:  none");
            } else {
                println!("Gateways:  {}", source.gateways.join(", "));
            }
        }
        SourceCommand::SwitchChannel { name, channel } => {
            validate_release_channel(&channel)?;
            let source = config
                .source_named_mut(name.as_deref())
                .ok_or_else(|| anyhow::anyhow!("Trusted source not found"))?;
            source.channel = channel.clone();
            if source.discovery_uri.is_empty()
                || source.discovery_uri.starts_with("elastos://source/")
            {
                if let Some(publisher) = source.publisher_dids.first() {
                    source.discovery_uri = source_discovery_uri_fn(publisher, &channel);
                }
            }
            let source_name = source.name.clone();
            save_trusted_sources(data_dir, &config)?;

            println!(
                "Trusted source '{}' now tracks channel '{}'.",
                source_name, channel
            );
        }
        SourceCommand::Verify { name } => {
            let source = config
                .source_named(name.as_deref())
                .ok_or_else(|| anyhow::anyhow!("Trusted source not found"))?;
            let binary = Path::new(&source.install_path);
            let parent = std::fs::canonicalize(
                binary
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("Installed binary parent is unavailable"))?,
            )?;
            let binary = parent.join(
                binary
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("Installed binary basename is unavailable"))?,
            );
            let guard = crate::install_transaction::InstallationGuard::acquire(&parent)?;
            let installed =
                crate::installed_release::load_or_migrate(data_dir, &binary, source, &guard)?;
            let head_path = installation_release_head_path(data_dir);
            if !head_path.exists() {
                anyhow::bail!("No local release head found at {}", head_path.display());
            }
            let head_bytes = installed.head;
            let (head, signer) = crate::crypto::verify_release_envelope_against_dids(
                &head_bytes,
                "elastos.release.head.v1",
                &source.publisher_dids,
            )?;
            let payload = &head["payload"];
            println!("Trusted source '{}' verified.", source.name);
            println!("  Signer:   {}", signer);
            println!(
                "  Channel:  {}",
                payload["channel"].as_str().unwrap_or("unknown")
            );
            println!(
                "  Version:  {}",
                payload["version"].as_str().unwrap_or("unknown")
            );
            println!(
                "  Release:  {}",
                payload["latest_release_cid"].as_str().unwrap_or("unknown")
            );
        }
    }

    Ok(())
}

fn validate_file_pins(cid: &str, sha256: &str, size: u64) -> anyhow::Result<()> {
    let parsed: cid::Cid = cid.parse()?;
    anyhow::ensure!(
        parsed.to_string() == cid,
        "Artifact CID must use its canonical spelling"
    );
    anyhow::ensure!(
        sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Artifact SHA-256 must be lowercase hexadecimal"
    );
    anyhow::ensure!(
        size > 0 && size <= 16 * 1024 * 1024 * 1024,
        "Artifact size must be 1..16 GiB"
    );
    Ok(())
}

async fn admit_file_download(
    file: &mut tokio::fs::File,
    sha256: &str,
    size: u64,
) -> anyhow::Result<()> {
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    anyhow::ensure!(
        file.metadata().await?.len() == size,
        "Artifact length differs from its pin"
    );
    file.rewind().await?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    anyhow::ensure!(
        hex::encode(digest.finalize()) == sha256,
        "Artifact checksum differs from its pin"
    );
    file.sync_all().await?;
    Ok(())
}

/// Build-time artifact acquisition uses the normal bounded Carrier file stream.
/// Publisher identity and independently supplied bytes pins remain separate inputs.
pub async fn fetch_source_file(
    data: &Path,
    name: &str,
    cid: &str,
    sha256: &str,
    size: u64,
    output: &Path,
) -> anyhow::Result<()> {
    validate_file_pins(cid, sha256, size)?;
    let config = load_trusted_sources(data)?;
    let source = config
        .sources
        .iter()
        .find(|source| source.name == name)
        .ok_or_else(|| anyhow::anyhow!("Named artifact source is absent"))?;
    let peer: iroh::PublicKey = source.publisher_node_id.parse()?;
    anyhow::ensure!(
        peer.to_string() == source.publisher_node_id,
        "Artifact source requires one canonical Carrier node ID"
    );
    anyhow::ensure!(
        !output.exists() && output.symlink_metadata().is_err(),
        "Artifact output already exists"
    );
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    crate::install_transaction::require_controller_update_space(parent, size)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut file = tokio::fs::File::from_std(temporary.as_file().try_clone()?);
    let mut displayed = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let mut checked = 0;
    let mut progress = |received: u64, total: u64| -> anyhow::Result<()> {
        if received == 0
            || received == total
            || received.saturating_sub(checked) >= 16 * 1024 * 1024
        {
            crate::install_transaction::require_controller_update_space(
                parent,
                total.saturating_sub(received),
            )?;
            checked = received;
        }
        if received == 0
            || received == total
            || displayed.elapsed() >= std::time::Duration::from_secs(1)
        {
            eprintln!("Artifact download: {received}/{total} bytes");
            displayed = std::time::Instant::now();
        }
        Ok(())
    };
    crate::carrier::fetch_file_from_trusted_source_to_bound(
        source,
        cid,
        &mut file,
        size,
        &mut progress,
        None,
    )
    .await?;
    admit_file_download(&mut file, sha256, size).await?;
    drop(file);
    temporary.persist_noclobber(output)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// The SourceCommand enum — CLI definition for `elastos source` subcommand.
/// Kept here so that the handler can reference it directly.
#[derive(clap::Subcommand)]
pub enum SourceCommand {
    /// Stream a pinned immutable artifact from an explicitly trusted Carrier source
    FetchFile {
        #[arg(long)]
        source: String,
        /// Canonical CID, served as the publisher's immutable artifact filename
        #[arg(long)]
        cid: String,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        size: u64,
        #[arg(long)]
        output: PathBuf,
    },
    /// Add or update a trusted release source
    Add {
        /// Source name (unique identifier)
        #[arg(long, default_value = "default")]
        name: String,
        /// Trusted publisher DID
        #[arg(long)]
        publisher: String,
        /// Release channel (keeps the existing channel, or defaults to stable for a new source)
        #[arg(long)]
        channel: Option<String>,
        /// Explicit ElastOS discovery URI for this source
        #[arg(long)]
        discovery_uri: Option<String>,
        /// Peer ticket used to bootstrap directly to the publisher
        #[arg(long)]
        connect_ticket: Option<String>,
        /// Preferred gateway URL (repeatable)
        #[arg(long = "gateway")]
        gateways: Vec<String>,
        /// Installed binary path for future updates
        #[arg(long)]
        install_path: Option<PathBuf>,
        /// Known HEAD CID for this source
        #[arg(long)]
        head_cid: Option<String>,
        /// Publisher's stable raw Iroh endpoint ID for durable P2P transport connections
        #[arg(long)]
        publisher_node_id: Option<String>,
        /// IPNS name for mutable release head pointer
        #[arg(long)]
        ipns_name: Option<String>,
    },
    /// List trusted release sources
    List,
    /// Show details for a trusted source
    Show {
        /// Source name (defaults to the current default source)
        name: Option<String>,
    },
    /// Change the subscribed channel for a source
    SwitchChannel {
        /// Source name (defaults to the current default source)
        name: Option<String>,
        /// New channel name
        channel: String,
    },
    /// Verify the locally saved release head against a trusted source
    Verify {
        /// Source name (defaults to the current default source)
        name: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_pins_require_canonical_content_identity_and_bounded_bytes() {
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, &[1u8; 32]).unwrap();
        let cid = cid::Cid::new_v1(0x55, multihash).to_string();
        let hash = "ab".repeat(32);
        validate_file_pins(&cid, &hash, 1).unwrap();
        for (id, checksum, size) in [
            ("../file", hash.as_str(), 1),
            (cid.as_str(), "ABCDEF", 1),
            (cid.as_str(), hash.as_str(), 0),
            (cid.as_str(), hash.as_str(), 16 * 1024 * 1024 * 1024 + 1),
        ] {
            assert!(validate_file_pins(id, checksum, size).is_err());
        }
    }

    #[tokio::test]
    async fn file_admission_checks_actual_bytes_and_size_before_publication() {
        use sha2::{Digest, Sha256};
        let temporary = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temporary.path(), b"verified artifact").unwrap();
        let mut file = tokio::fs::File::from_std(temporary.as_file().try_clone().unwrap());
        let hash = hex::encode(Sha256::digest(b"verified artifact"));
        admit_file_download(&mut file, &hash, 17).await.unwrap();
        assert!(admit_file_download(&mut file, &hash, 18).await.is_err());
        assert!(admit_file_download(&mut file, &"00".repeat(32), 17)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn file_fetch_requires_an_explicit_named_source_before_creating_output() {
        let dir = tempfile::tempdir().unwrap();
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, &[1u8; 32]).unwrap();
        let cid = cid::Cid::new_v1(0x55, multihash).to_string();
        let output = dir.path().join("artifact");
        let error = fetch_source_file(dir.path(), "missing", &cid, &"00".repeat(32), 1, &output)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Named artifact source is absent"));
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    // RFC 8032 public DID and a disposable pre-signed envelope from install-bootstrap-test.py.
    // These tests use public verification data only.
    const OLD_DID: &str = "did:key:z6MktwupdmLXVVqTzCw4i46r4uGyosGXRnR3XjN4Zq7oMMsw";
    const NEW_DID: &str = "did:key:z6MkvDqGT54cXesYGvABpF1UapVNwjCqRcafi4Px6Thv5T3Z";

    fn discovery_uri(publisher: &str, channel: &str) -> String {
        format!("elastos://source/{channel}/{publisher}")
    }

    fn fixture_source() -> TrustedSource {
        TrustedSource {
            name: "fixture".into(),
            publisher_dids: vec![OLD_DID.into()],
            channel: "canary".into(),
            discovery_uri: discovery_uri(OLD_DID, "canary"),
            connect_ticket: "fixture-ticket".into(),
            gateways: vec!["https://fixture.invalid".into()],
            install_path: "/fixture/bin/elastos".into(),
            installed_version: "0.7.1".into(),
            head_cid: "fixture-head".into(),
            publisher_node_id: "fixture-delivery-node".into(),
            ipns_name: "fixture-ipns".into(),
        }
    }

    fn source_add(publisher: &str) -> SourceCommand {
        SourceCommand::Add {
            name: "fixture".into(),
            publisher: publisher.into(),
            channel: None,
            discovery_uri: None,
            connect_ticket: None,
            gateways: Vec::new(),
            install_path: None,
            head_cid: None,
            publisher_node_id: None,
            ipns_name: None,
        }
    }

    fn save_fixture(data_dir: &Path, source: TrustedSource) {
        let mut config = TrustedSourcesConfig::empty();
        config.upsert_source(source);
        let mut other = fixture_source();
        other.name = "other".into();
        config.upsert_source(other);
        config.default_source = "other".into();
        save_trusted_sources(data_dir, &config).unwrap();
    }

    #[test]
    fn retrust_preserves_settings_and_other_sources_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        save_fixture(dir.path(), fixture_source());
        let before = load_trusted_sources(dir.path()).unwrap();
        run_source_command_with_confirmation(
            source_add(NEW_DID),
            discovery_uri,
            dir.path(),
            |old, new| {
                assert_eq!(old.publisher_dids, [OLD_DID]);
                assert_eq!(new.publisher_dids, [NEW_DID]);
                Ok(true)
            },
        )
        .unwrap();
        let after = load_trusted_sources(dir.path()).unwrap();
        let mut expected = serde_json::to_value(&before).unwrap();
        expected["sources"][0]["publisher_dids"] = serde_json::json!([NEW_DID]);
        expected["sources"][0]["discovery_uri"] = discovery_uri(NEW_DID, "canary").into();
        assert_eq!(serde_json::to_value(after).unwrap(), expected);
    }

    #[test]
    fn retrust_cancel_eof_and_wrong_confirmation_preserve_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        save_fixture(dir.path(), fixture_source());
        let before = std::fs::read(trusted_sources_path(dir.path())).unwrap();
        for response in ["\n", "", OLD_DID, "yes\n"] {
            let mut input = std::io::Cursor::new(response.as_bytes());
            let mut output = Vec::new();
            let result = run_source_command_with_confirmation(
                source_add(NEW_DID),
                discovery_uri,
                dir.path(),
                |old, new| confirm_signer_change_with_io(old, new, &mut input, &mut output),
            );
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            assert_eq!(
                std::fs::read(trusted_sources_path(dir.path())).unwrap(),
                before
            );
            let prompt = String::from_utf8(output).unwrap();
            assert!(prompt.contains(OLD_DID));
            assert!(prompt.contains(NEW_DID));
        }
    }

    #[test]
    fn retrust_accepts_only_exact_public_did_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        save_fixture(dir.path(), fixture_source());
        let mut input = std::io::Cursor::new(format!("{NEW_DID}\n"));
        run_source_command_with_confirmation(
            source_add(NEW_DID),
            discovery_uri,
            dir.path(),
            |old, new| confirm_signer_change_with_io(old, new, &mut input, &mut Vec::new()),
        )
        .unwrap();
        assert_eq!(
            load_trusted_sources(dir.path()).unwrap().sources[0].publisher_dids,
            [NEW_DID]
        );
    }

    #[test]
    fn retrust_refuses_invalid_did_and_channel_before_confirmation_or_write() {
        let dir = tempfile::tempdir().unwrap();
        save_fixture(dir.path(), fixture_source());
        let before = std::fs::read(trusted_sources_path(dir.path())).unwrap();
        for publisher in [
            "did:key:invalid".to_string(),
            format!("{NEW_DID}#key"),
            format!(" {NEW_DID}"),
            format!("{OLD_DID},{NEW_DID}"),
        ] {
            assert!(run_source_command_with_confirmation(
                source_add(&publisher),
                discovery_uri,
                dir.path(),
                |_, _| panic!("invalid DID reached confirmation")
            )
            .is_err());
            assert_eq!(
                std::fs::read(trusted_sources_path(dir.path())).unwrap(),
                before
            );
        }
        for channel_value in ["nightly", ""] {
            let mut cmd = source_add(NEW_DID);
            if let SourceCommand::Add { channel, .. } = &mut cmd {
                *channel = Some(channel_value.into());
            }
            assert!(run_source_command_with_confirmation(
                cmd,
                discovery_uri,
                dir.path(),
                |_, _| panic!("invalid channel reached confirmation")
            )
            .is_err());
            assert_eq!(
                std::fs::read(trusted_sources_path(dir.path())).unwrap(),
                before
            );
        }
    }

    #[test]
    fn retrust_preserves_custom_discovery_and_applies_explicit_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture_source();
        source.discovery_uri = "elastos://custom/fixture".into();
        save_fixture(dir.path(), source);
        run_source_command_with_confirmation(
            source_add(NEW_DID),
            discovery_uri,
            dir.path(),
            |_, _| Ok(true),
        )
        .unwrap();
        assert_eq!(
            load_trusted_sources(dir.path()).unwrap().sources[0].discovery_uri,
            "elastos://custom/fixture"
        );
        let mut cmd = source_add(NEW_DID);
        if let SourceCommand::Add {
            channel,
            discovery_uri,
            connect_ticket,
            gateways,
            install_path,
            head_cid,
            publisher_node_id,
            ipns_name,
            ..
        } = &mut cmd
        {
            *channel = Some("jetson-test".into());
            *discovery_uri = Some("elastos://custom/replacement".into());
            *connect_ticket = Some("replacement-ticket".into());
            *gateways = vec![" https://replacement.invalid/ ".into()];
            *install_path = Some(PathBuf::from("/fixture/new/elastos"));
            *head_cid = Some("replacement-head".into());
            *publisher_node_id = Some("replacement-delivery-node".into());
            *ipns_name = Some("replacement-ipns".into());
        }
        run_source_command_with_confirmation(cmd, discovery_uri, dir.path(), |_, _| {
            panic!("unchanged signer reached confirmation")
        })
        .unwrap();
        let reloaded = load_trusted_sources(dir.path()).unwrap();
        let source = &reloaded.sources[0];
        assert_eq!(source.channel, "jetson-test");
        assert_eq!(source.discovery_uri, "elastos://custom/replacement");
        assert_eq!(source.connect_ticket, "replacement-ticket");
        assert_eq!(source.gateways, ["https://replacement.invalid"]);
        assert_eq!(source.install_path, "/fixture/new/elastos");
        assert_eq!(source.head_cid, "replacement-head");
        assert_eq!(source.publisher_node_id, "replacement-delivery-node");
        assert_eq!(source.ipns_name, "replacement-ipns");
        assert_eq!(source.installed_version, "0.7.1");
    }

    #[test]
    fn retrust_fresh_source_defaults_stable_and_existing_legacy_channel_stays_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = source_add(NEW_DID);
        if let SourceCommand::Add { install_path, .. } = &mut cmd {
            *install_path = Some(dir.path().join("bin/elastos"));
        }
        run_source_command_with_confirmation(cmd, discovery_uri, dir.path(), |_, _| {
            panic!("fresh source reached confirmation")
        })
        .unwrap();
        let source = load_trusted_sources(dir.path()).unwrap().sources.remove(0);
        assert_eq!(source.channel, "stable");
        assert_eq!(source.discovery_uri, discovery_uri(NEW_DID, "stable"));
        let mut source = fixture_source();
        source.channel.clear();
        save_fixture(dir.path(), source);
        run_source_command_with_confirmation(
            source_add(NEW_DID),
            discovery_uri,
            dir.path(),
            |_, _| Ok(true),
        )
        .unwrap();
        assert_eq!(
            load_trusted_sources(dir.path()).unwrap().sources[0].channel,
            ""
        );
    }

    #[test]
    fn retrust_replaces_multiple_signers_with_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture_source();
        source.publisher_dids.push(NEW_DID.into());
        save_fixture(dir.path(), source);
        run_source_command_with_confirmation(
            source_add(NEW_DID),
            discovery_uri,
            dir.path(),
            |_, _| Ok(true),
        )
        .unwrap();
        assert_eq!(
            load_trusted_sources(dir.path()).unwrap().sources[0].publisher_dids,
            [NEW_DID]
        );
    }

    #[test]
    fn retrust_accepts_new_public_signature_and_refuses_old_anchor_before_signature_check() {
        let dir = tempfile::tempdir().unwrap();
        save_fixture(dir.path(), fixture_source());
        let signed_release = serde_json::to_vec(&serde_json::json!({
            "payload": {"schema": "elastos.release/v1", "version": "0.7.1", "channel": "stable",
                "platforms": {"x86_64-linux": {
                    "binary": {"cid": "binary-a", "sha256": "a".repeat(64)},
                    "components": {"cid": "components", "sha256": "b".repeat(64)}}}},
            "signer_did": NEW_DID,
            "signature": "e976be583f98da06863071e4f2006dc2ea97fd77451fbc278ef23fcc3e1f97bad27abe6f617c11b262994838b49896b2ded44b57531f62d3bcea70e2cefd2b06"
        })).unwrap();
        run_source_command_with_confirmation(
            source_add(NEW_DID),
            discovery_uri,
            dir.path(),
            |_, _| Ok(true),
        )
        .unwrap();
        let persisted = std::fs::read(trusted_sources_path(dir.path())).unwrap();
        let config = load_trusted_sources(dir.path()).unwrap();
        let trusted_dids = &config.sources[0].publisher_dids;
        assert_eq!(trusted_dids, &[NEW_DID]);
        crate::crypto::verify_release_envelope_against_dids(
            &signed_release,
            "elastos.release.v1",
            trusted_dids,
        )
        .unwrap();

        // An independently signed OLD_DID fixture is unavailable. This envelope
        // proves that the old anchor is refused before signature validation;
        // the NEW_DID envelope above separately proves signature acceptance.
        let old_signer_envelope = serde_json::to_vec(&serde_json::json!({
            "signer_did": OLD_DID,
            "payload": {},
            "signature": "invalid-signature"
        }))
        .unwrap();
        let error = crate::crypto::verify_release_envelope_against_dids(
            &old_signer_envelope,
            "elastos.release.v1",
            trusted_dids,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Signer DID mismatch"));
        assert_eq!(
            std::fs::read(trusted_sources_path(dir.path())).unwrap(),
            persisted
        );
    }

    #[test]
    fn test_validate_release_channel_accepts_supported_channels() {
        for channel in ["stable", "canary", "jetson-test"] {
            validate_release_channel(channel).unwrap();
        }
    }

    #[test]
    fn test_validate_release_channel_rejects_unknown_channels() {
        let err = validate_release_channel("nightly").unwrap_err();
        assert!(err.to_string().contains("Allowed channels"));
    }
}
