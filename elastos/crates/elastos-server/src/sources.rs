//! Trusted release sources: configuration, persistence, and CLI handlers.

use std::path::PathBuf;

use elastos_common::localhost::publisher_release_head_path;

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
    /// Stable Iroh node ID (derived from publisher's device key) for durable P2P connections.
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
    let mut config = load_trusted_sources(&data_dir)?;

    match cmd {
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
            validate_release_channel(&channel)?;
            let resolved_discovery_uri = discovery_uri
                .filter(|uri| !uri.trim().is_empty())
                .unwrap_or_else(|| source_discovery_uri_fn(&publisher, &channel));
            let source = TrustedSource {
                name: name.clone(),
                publisher_dids: vec![publisher],
                channel,
                discovery_uri: resolved_discovery_uri,
                connect_ticket: connect_ticket.unwrap_or_default(),
                gateways: normalize_gateways(&gateways),
                install_path: install_path
                    .unwrap_or_else(infer_install_path)
                    .display()
                    .to_string(),
                installed_version: config
                    .source_named(Some(&name))
                    .map(|s| s.installed_version.clone())
                    .unwrap_or_default(),
                head_cid: head_cid.unwrap_or_else(|| {
                    config
                        .source_named(Some(&name))
                        .map(|s| s.head_cid.clone())
                        .unwrap_or_default()
                }),
                publisher_node_id: publisher_node_id.unwrap_or_else(|| {
                    config
                        .source_named(Some(&name))
                        .map(|s| s.publisher_node_id.clone())
                        .unwrap_or_default()
                }),
                ipns_name: ipns_name.unwrap_or_else(|| {
                    config
                        .source_named(Some(&name))
                        .map(|s| s.ipns_name.clone())
                        .unwrap_or_default()
                }),
            };
            config.upsert_source(source);
            if config.default_source.is_empty() {
                config.default_source = name.clone();
            }
            save_trusted_sources(&data_dir, &config)?;

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
            save_trusted_sources(&data_dir, &config)?;

            println!(
                "Trusted source '{}' now tracks channel '{}'.",
                source_name, channel
            );
        }
        SourceCommand::Verify { name } => {
            let source = config
                .source_named(name.as_deref())
                .ok_or_else(|| anyhow::anyhow!("Trusted source not found"))?;
            let head_path = publisher_release_head_path(&data_dir);
            if !head_path.exists() {
                anyhow::bail!("No local release head found at {}", head_path.display());
            }
            let head_bytes = std::fs::read(&head_path)?;
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

/// The SourceCommand enum — CLI definition for `elastos source` subcommand.
/// Kept here so that the handler can reference it directly.
#[derive(clap::Subcommand)]
pub enum SourceCommand {
    /// Add or update a trusted release source
    Add {
        /// Source name (unique identifier)
        #[arg(long, default_value = "default")]
        name: String,
        /// Trusted publisher DID
        #[arg(long)]
        publisher: String,
        /// Release channel name
        #[arg(long, default_value = "stable")]
        channel: String,
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
        /// Publisher's stable Iroh node ID for durable P2P connections
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
    use super::validate_release_channel;

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
