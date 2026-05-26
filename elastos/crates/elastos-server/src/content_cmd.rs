use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::Subcommand;

#[derive(Subcommand)]
pub enum ContentCommand {
    /// Publish an IPLD-compatible content object through the content provider
    #[command(name = "publish-object")]
    PublishObject {
        /// File or directory to publish
        path: PathBuf,

        /// Content object kind for _elastos_object.json
        #[arg(long, default_value = "directory")]
        kind: String,

        /// Entry name to use when PATH is a single file
        #[arg(long)]
        entry_name: Option<String>,

        /// Stable object identity, when known
        #[arg(long)]
        object_did: Option<String>,

        /// Publisher identity, when known
        #[arg(long)]
        publisher_did: Option<String>,

        /// Link in rel=cid form; may be repeated
        #[arg(long = "link")]
        links: Vec<String>,
    },
}

pub async fn run(cmd: ContentCommand) -> anyhow::Result<()> {
    match cmd {
        ContentCommand::PublishObject {
            path,
            kind,
            entry_name,
            object_did,
            publisher_did,
            links,
        } => {
            let links = parse_links(&links)?;
            let registry = crate::get_content_registry().await?;
            let cid = if path.is_dir() {
                if entry_name.is_some() {
                    anyhow::bail!("--entry-name is only valid when publishing a single file");
                }
                publish_object_dir(
                    &registry,
                    &path,
                    &kind,
                    object_did.as_deref(),
                    publisher_did.as_deref(),
                    &links,
                )
                .await?
            } else if path.is_file() {
                let entry_name = match entry_name {
                    Some(value) => validate_entry_name(&value)?.to_string(),
                    None => path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                        .ok_or_else(|| anyhow::anyhow!("file path has no valid entry name"))?,
                };
                publish_object_file(
                    &registry,
                    &path,
                    &entry_name,
                    &kind,
                    object_did.as_deref(),
                    publisher_did.as_deref(),
                    &links,
                )
                .await?
            } else {
                anyhow::bail!("content object path does not exist: {}", path.display());
            };
            println!("{cid}");
        }
    }
    Ok(())
}

async fn publish_object_dir(
    registry: &elastos_runtime::provider::ProviderRegistry,
    path: &Path,
    kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    links: &[(String, String)],
) -> anyhow::Result<String> {
    elastos_server::content::publish_directory_via_provider_with_kind_and_links(
        registry,
        path,
        kind,
        object_did,
        publisher_did,
        links,
    )
    .await
}

async fn publish_object_file(
    registry: &elastos_runtime::provider::ProviderRegistry,
    path: &Path,
    entry_name: &str,
    kind: &str,
    object_did: Option<&str>,
    publisher_did: Option<&str>,
    links: &[(String, String)],
) -> anyhow::Result<String> {
    let temp_dir = tempfile::Builder::new()
        .prefix("elastos-content-object-")
        .tempdir()?;
    std::fs::copy(path, temp_dir.path().join(entry_name))?;
    publish_object_dir(
        registry,
        temp_dir.path(),
        kind,
        object_did,
        publisher_did,
        links,
    )
    .await
}

fn parse_links(values: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    let mut seen = BTreeSet::new();
    let mut links = Vec::with_capacity(values.len());
    for value in values {
        let (rel, cid) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("content object links must use rel=cid"))?;
        let rel = validate_link_rel(rel)?;
        let cid = cid.trim();
        if cid.is_empty() {
            anyhow::bail!("content object link '{rel}' is missing cid");
        }
        cid::Cid::try_from(cid)
            .map_err(|err| anyhow::anyhow!("content object link '{rel}' has invalid cid: {err}"))?;
        let key = (rel.to_string(), cid.to_string());
        if !seen.insert(key.clone()) {
            anyhow::bail!("duplicate content object link {rel}={cid}");
        }
        links.push(key);
    }
    links.sort();
    Ok(links)
}

fn validate_link_rel(rel: &str) -> anyhow::Result<&str> {
    let rel = rel.trim();
    if rel.is_empty() {
        anyhow::bail!("content object link rel must not be empty");
    }
    if rel.len() > 80 {
        anyhow::bail!("content object link rel is too long");
    }
    if !rel
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_' | '.'))
    {
        anyhow::bail!("content object link rel must use lowercase ascii, digits, '-', '_', or '.'");
    }
    Ok(rel)
}

fn validate_entry_name(value: &str) -> anyhow::Result<&str> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        anyhow::bail!("single-file content object entry name must be a plain file name");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CID: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";

    #[test]
    fn content_command_parses_sorted_links() {
        let links = parse_links(&[
            format!("release={TEST_CID}"),
            format!("binary.x86_64-linux={TEST_CID}"),
        ])
        .unwrap();

        assert_eq!(links[0].0, "binary.x86_64-linux");
        assert_eq!(links[1].0, "release");
    }

    #[test]
    fn content_command_rejects_ambiguous_file_entry() {
        assert!(validate_entry_name("release.json").is_ok());
        assert!(validate_entry_name("../release.json").is_err());
        assert!(validate_entry_name("nested/release.json").is_err());
    }
}
