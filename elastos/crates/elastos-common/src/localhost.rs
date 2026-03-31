//! Canonical localhost:// roots for the WCI-aligned local PC2 model.

use std::path::{Path, PathBuf};

pub const MY_WEBSITE_URI: &str = "localhost://MyWebSite";
pub const PUBLISHER_ROOT_URI: &str = "localhost://ElastOS/SystemServices/Publisher";
pub const EDGE_ROOT_URI: &str = "localhost://ElastOS/SystemServices/Edge";

pub const FILE_BACKED_ROOTS: &[&str] = &[
    "AppCapsules",
    "ElastOS",
    "Local",
    "MyWebSite",
    "PC2Host",
    "Public",
    "Users",
    "UsersAI",
];

pub const DYNAMIC_ROOTS: &[&str] = &["WebSpaces"];

pub const ALL_ROOTS: &[&str] = &[
    "AppCapsules",
    "ElastOS",
    "Local",
    "MyWebSite",
    "PC2Host",
    "Public",
    "Users",
    "UsersAI",
    "WebSpaces",
];

pub fn is_supported_resource_scheme(uri: &str) -> bool {
    uri.starts_with("elastos://") || uri.starts_with("localhost://")
}

pub fn parse_localhost_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("localhost://")?;
    parse_localhost_path(rest)
}

pub fn parse_localhost_path(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim().trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    match trimmed.find('/') {
        Some(idx) => {
            let root = &trimmed[..idx];
            let remainder = &trimmed[idx + 1..];
            ALL_ROOTS.contains(&root).then_some((root, remainder))
        }
        None => ALL_ROOTS.contains(&trimmed).then_some((trimmed, "")),
    }
}

pub fn rooted_localhost_uri(path_or_uri: &str) -> Option<String> {
    if let Some((root, rest)) = parse_localhost_uri(path_or_uri) {
        return Some(if rest.is_empty() {
            format!("localhost://{}", root)
        } else {
            format!("localhost://{}/{}", root, rest)
        });
    }

    let (root, rest) = parse_localhost_path(path_or_uri)?;
    Some(if rest.is_empty() {
        format!("localhost://{}", root)
    } else {
        format!("localhost://{}/{}", root, rest)
    })
}

pub fn is_file_backed_root(root: &str) -> bool {
    FILE_BACKED_ROOTS.contains(&root)
}

pub fn is_plaintext_root(root: &str) -> bool {
    matches!(root, "MyWebSite" | "Public")
}

pub fn file_backed_prefixes() -> Vec<String> {
    FILE_BACKED_ROOTS
        .iter()
        .map(|root| (*root).to_string())
        .collect()
}

pub fn ensure_file_backed_roots(base_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    for root in FILE_BACKED_ROOTS {
        let path = base_dir.join(root);
        std::fs::create_dir_all(&path)?;
        created.push(path);
    }
    Ok(created)
}

pub fn rooted_localhost_fs_path(base_dir: &Path, path_or_uri: &str) -> Option<PathBuf> {
    let (root, rest) = if path_or_uri.starts_with("localhost://") {
        parse_localhost_uri(path_or_uri)?
    } else {
        parse_localhost_path(path_or_uri)?
    };
    if !is_file_backed_root(root) {
        return None;
    }

    let mut path = base_dir.join(root);
    if !rest.is_empty() {
        for segment in rest.split('/') {
            if segment.is_empty() {
                continue;
            }
            if segment == "." || segment == ".." || segment.contains('\\') {
                return None;
            }
            path.push(segment);
        }
    }
    Some(path)
}

pub fn my_website_root_path(base_dir: &Path) -> PathBuf {
    rooted_localhost_fs_path(base_dir, MY_WEBSITE_URI).expect("MyWebSite must be file-backed")
}

pub fn publisher_root_path(base_dir: &Path) -> PathBuf {
    rooted_localhost_fs_path(base_dir, PUBLISHER_ROOT_URI).expect("Publisher root must exist")
}

pub fn publisher_artifacts_path(base_dir: &Path) -> PathBuf {
    publisher_root_path(base_dir).join("artifacts")
}

pub fn publisher_site_releases_root_path(base_dir: &Path) -> PathBuf {
    publisher_root_path(base_dir).join("SiteReleases")
}

pub fn publisher_release_head_path(base_dir: &Path) -> PathBuf {
    publisher_root_path(base_dir).join("release-head.json")
}

pub fn publisher_release_manifest_path(base_dir: &Path) -> PathBuf {
    publisher_root_path(base_dir).join("release.json")
}

pub fn publisher_install_script_path(base_dir: &Path) -> PathBuf {
    publisher_root_path(base_dir).join("install.sh")
}

pub fn publisher_publish_state_path(base_dir: &Path) -> PathBuf {
    publisher_root_path(base_dir).join("publish-state.json")
}

pub fn publisher_site_releases_dir(base_dir: &Path, target_uri: &str) -> PathBuf {
    publisher_site_releases_root_path(base_dir).join(sanitize_edge_state_name(target_uri))
}

pub fn publisher_site_release_path(
    base_dir: &Path,
    target_uri: &str,
    release_name: &str,
) -> PathBuf {
    publisher_site_releases_dir(base_dir, target_uri)
        .join(format!("{}.json", sanitize_edge_state_name(release_name)))
}

pub fn edge_root_path(base_dir: &Path) -> PathBuf {
    rooted_localhost_fs_path(base_dir, EDGE_ROOT_URI).expect("Edge root must exist")
}

pub fn edge_bindings_path(base_dir: &Path) -> PathBuf {
    edge_root_path(base_dir).join("Bindings")
}

pub fn edge_site_heads_path(base_dir: &Path) -> PathBuf {
    edge_root_path(base_dir).join("SiteHeads")
}

pub fn edge_release_channels_root_path(base_dir: &Path) -> PathBuf {
    edge_root_path(base_dir).join("ReleaseChannels")
}

pub fn edge_site_history_root_path(base_dir: &Path) -> PathBuf {
    edge_root_path(base_dir).join("SiteHistory")
}

pub fn sanitize_edge_state_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub fn sanitize_edge_binding_name(domain: &str) -> String {
    sanitize_edge_state_name(domain.trim().trim_end_matches('.'))
}

pub fn edge_binding_path(base_dir: &Path, domain: &str) -> PathBuf {
    edge_bindings_path(base_dir).join(format!("{}.json", sanitize_edge_binding_name(domain)))
}

pub fn edge_site_head_path(base_dir: &Path, target_uri: &str) -> PathBuf {
    edge_site_heads_path(base_dir).join(format!("{}.json", sanitize_edge_state_name(target_uri)))
}

pub fn edge_site_history_dir(base_dir: &Path, target_uri: &str) -> PathBuf {
    edge_site_history_root_path(base_dir).join(sanitize_edge_state_name(target_uri))
}

pub fn edge_release_channels_dir(base_dir: &Path, target_uri: &str) -> PathBuf {
    edge_release_channels_root_path(base_dir).join(sanitize_edge_state_name(target_uri))
}

pub fn edge_release_channel_path(base_dir: &Path, target_uri: &str, channel_name: &str) -> PathBuf {
    edge_release_channels_dir(base_dir, target_uri)
        .join(format!("{}.json", sanitize_edge_state_name(channel_name)))
}

pub fn ensure_system_service_roots(base_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut created = ensure_file_backed_roots(base_dir)?;
    for path in [
        publisher_artifacts_path(base_dir),
        publisher_site_releases_root_path(base_dir),
        edge_bindings_path(base_dir),
        edge_site_heads_path(base_dir),
        edge_release_channels_root_path(base_dir),
        edge_site_history_root_path(base_dir),
        my_website_root_path(base_dir),
    ] {
        std::fs::create_dir_all(&path)?;
        created.push(path);
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_localhost_uri() {
        assert_eq!(
            parse_localhost_uri("localhost://Users/self/Documents"),
            Some(("Users", "self/Documents"))
        );
        assert_eq!(
            parse_localhost_uri("localhost://MyWebSite"),
            Some(("MyWebSite", ""))
        );
        assert_eq!(parse_localhost_uri("localhost://UnknownRoot/test"), None);
    }

    #[test]
    fn test_rooted_localhost_uri() {
        assert_eq!(
            rooted_localhost_uri("Users/self/.AppData/LocalHost/Chat/history.json"),
            Some("localhost://Users/self/.AppData/LocalHost/Chat/history.json".to_string())
        );
        assert_eq!(
            rooted_localhost_uri("localhost://Public/demo.txt"),
            Some("localhost://Public/demo.txt".to_string())
        );
        assert_eq!(rooted_localhost_uri("chat/history.json"), None);
    }

    #[test]
    fn test_rooted_localhost_fs_path() {
        let base = Path::new("/tmp/elastos");
        assert_eq!(
            rooted_localhost_fs_path(base, "localhost://ElastOS/SystemServices/Publisher").unwrap(),
            PathBuf::from("/tmp/elastos/ElastOS/SystemServices/Publisher")
        );
        assert_eq!(
            rooted_localhost_fs_path(base, "Users/self/Documents").unwrap(),
            PathBuf::from("/tmp/elastos/Users/self/Documents")
        );
        assert!(rooted_localhost_fs_path(base, "localhost://Public/../Users").is_none());
    }

    #[test]
    fn test_edge_binding_path() {
        let base = Path::new("/tmp/elastos");
        assert_eq!(
            edge_binding_path(base, "Elastos.ElacityLabs.com:443"),
            PathBuf::from(
                "/tmp/elastos/ElastOS/SystemServices/Edge/Bindings/elastos.elacitylabs.com_443.json"
            )
        );
    }

    #[test]
    fn test_edge_site_head_path() {
        let base = Path::new("/tmp/elastos");
        assert_eq!(
            edge_site_head_path(base, "localhost://MyWebSite"),
            PathBuf::from(
                "/tmp/elastos/ElastOS/SystemServices/Edge/SiteHeads/localhost___mywebsite.json"
            )
        );
    }

    #[test]
    fn test_edge_site_history_dir() {
        let base = Path::new("/tmp/elastos");
        assert_eq!(
            edge_site_history_dir(base, "localhost://MyWebSite"),
            PathBuf::from(
                "/tmp/elastos/ElastOS/SystemServices/Edge/SiteHistory/localhost___mywebsite"
            )
        );
    }

    #[test]
    fn test_edge_release_channel_path() {
        let base = Path::new("/tmp/elastos");
        assert_eq!(
            edge_release_channel_path(base, "localhost://MyWebSite", "live"),
            PathBuf::from(
                "/tmp/elastos/ElastOS/SystemServices/Edge/ReleaseChannels/localhost___mywebsite/live.json"
            )
        );
    }

    #[test]
    fn test_publisher_site_release_path() {
        let base = Path::new("/tmp/elastos");
        assert_eq!(
            publisher_site_release_path(base, "localhost://MyWebSite", "weekend-demo"),
            PathBuf::from(
                "/tmp/elastos/ElastOS/SystemServices/Publisher/SiteReleases/localhost___mywebsite/weekend-demo.json"
            )
        );
    }
}
