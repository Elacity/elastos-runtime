use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use elastos_common::localhost::edge_browser_app_path;
use serde::{Deserialize, Serialize};

use crate::sources::load_trusted_sources;

const HOSTED_ENDPOINT_SCHEMA: &str = "elastos.browser-app-hosts/v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserAppHostedEndpointSummary {
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub canonical_url: Option<String>,
    #[serde(default)]
    pub ephemeral_url: Option<String>,
    #[serde(default)]
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserAppHostedEndpointRecord {
    schema: String,
    app: String,
    #[serde(default)]
    canonical_url: Option<String>,
    #[serde(default)]
    ephemeral_url: Option<String>,
    #[serde(default)]
    updated_at: Option<u64>,
}

impl Default for BrowserAppHostedEndpointRecord {
    fn default() -> Self {
        Self {
            schema: HOSTED_ENDPOINT_SCHEMA.to_string(),
            app: String::new(),
            canonical_url: None,
            ephemeral_url: None,
            updated_at: None,
        }
    }
}

pub fn load_browser_app_hosted_endpoint(
    data_dir: &Path,
    app: &str,
) -> anyhow::Result<BrowserAppHostedEndpointSummary> {
    let path = edge_browser_app_path(data_dir, app);
    let mut record = if path.exists() {
        let bytes = std::fs::read(&path)?;
        let mut record: BrowserAppHostedEndpointRecord = serde_json::from_slice(&bytes)?;
        if record.schema.trim().is_empty() {
            record.schema = HOSTED_ENDPOINT_SCHEMA.to_string();
        }
        record
    } else {
        BrowserAppHostedEndpointRecord {
            app: app.to_string(),
            ..BrowserAppHostedEndpointRecord::default()
        }
    };

    let canonical_url = canonical_browser_app_url(data_dir, app);
    if canonical_url.is_some() {
        record.canonical_url = canonical_url;
    }

    Ok(BrowserAppHostedEndpointSummary {
        app: app.to_string(),
        canonical_url: record.canonical_url,
        ephemeral_url: record.ephemeral_url,
        updated_at: record.updated_at,
    })
}

pub fn record_ephemeral_browser_app_url(
    data_dir: &Path,
    app: &str,
    ephemeral_url: Option<&str>,
) -> anyhow::Result<()> {
    let path = edge_browser_app_path(data_dir, app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let canonical_url = canonical_browser_app_url(data_dir, app);
    let record = BrowserAppHostedEndpointRecord {
        schema: HOSTED_ENDPOINT_SCHEMA.to_string(),
        app: app.to_string(),
        canonical_url,
        ephemeral_url: ephemeral_url.map(normalize_public_url),
        updated_at: Some(now_ts()),
    };
    std::fs::write(&path, serde_json::to_vec_pretty(&record)?)?;
    Ok(())
}

pub fn record_ephemeral_browser_app_urls<'a>(
    data_dir: &Path,
    apps: impl IntoIterator<Item = &'a str>,
    ephemeral_url: Option<&str>,
) -> anyhow::Result<Vec<BrowserAppHostedEndpointSummary>> {
    let mut summaries = Vec::new();
    for app in apps {
        record_ephemeral_browser_app_url(data_dir, app, ephemeral_url)?;
        summaries.push(load_browser_app_hosted_endpoint(data_dir, app)?);
    }
    Ok(summaries)
}

fn canonical_browser_app_url(data_dir: &Path, app: &str) -> Option<String> {
    let env_gateway = std::env::var("ELASTOS_CANONICAL_PUBLISHER_GATEWAY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("ELASTOS_PUBLISHER_GATEWAY")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    if let Some(gateway) = env_gateway {
        return Some(format!(
            "{}/apps/{}/",
            gateway.trim_end_matches('/'),
            app.trim_matches('/')
        ));
    }

    let config = load_trusted_sources(data_dir).ok()?;
    let source = config.default_source()?;
    let gateway = source.gateways.first()?;
    Some(format!(
        "{}/apps/{}/",
        gateway.trim_end_matches('/'),
        app.trim_matches('/')
    ))
}

fn normalize_public_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};

    #[test]
    fn canonical_browser_app_url_uses_default_trusted_source_gateway() {
        let tmp = tempfile::tempdir().unwrap();
        save_trusted_sources(
            tmp.path(),
            &TrustedSourcesConfig {
                schema: "elastos.trusted-sources/v1".to_string(),
                default_source: "default".to_string(),
                sources: vec![TrustedSource {
                    name: "default".to_string(),
                    publisher_dids: vec![],
                    channel: "stable".to_string(),
                    discovery_uri: String::new(),
                    connect_ticket: String::new(),
                    gateways: vec!["https://elastos.elacitylabs.com".to_string()],
                    install_path: String::new(),
                    installed_version: String::new(),
                    head_cid: String::new(),
                    publisher_node_id: String::new(),
                    ipns_name: String::new(),
                }],
            },
        )
        .unwrap();

        let summary = load_browser_app_hosted_endpoint(tmp.path(), "demo-app").unwrap();
        assert_eq!(
            summary.canonical_url.as_deref(),
            Some("https://elastos.elacitylabs.com/apps/demo-app/")
        );
        assert_eq!(summary.ephemeral_url, None);
    }

    #[test]
    fn recording_ephemeral_browser_app_url_persists_edge_state() {
        let tmp = tempfile::tempdir().unwrap();
        record_ephemeral_browser_app_url(
            tmp.path(),
            "demo-app",
            Some("https://demo.trycloudflare.com"),
        )
        .unwrap();

        let summary = load_browser_app_hosted_endpoint(tmp.path(), "demo-app").unwrap();
        assert_eq!(
            summary.ephemeral_url.as_deref(),
            Some("https://demo.trycloudflare.com/")
        );
        assert!(summary.updated_at.is_some());
    }

    #[test]
    fn recording_ephemeral_browser_app_urls_updates_each_app() {
        let tmp = tempfile::tempdir().unwrap();
        let summaries = record_ephemeral_browser_app_urls(
            tmp.path(),
            ["chat-room", "home"],
            Some("https://demo.trycloudflare.com"),
        )
        .unwrap();

        assert_eq!(summaries.len(), 2);
        for app in ["chat-room", "home"] {
            let summary = load_browser_app_hosted_endpoint(tmp.path(), app).unwrap();
            assert_eq!(
                summary.ephemeral_url.as_deref(),
                Some("https://demo.trycloudflare.com/")
            );
        }
    }
}
