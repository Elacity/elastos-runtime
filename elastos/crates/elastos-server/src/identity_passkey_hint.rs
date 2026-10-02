//! Read existing public authentication metadata without loading identity stores.
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CREDENTIAL_CHARS: usize = 1366; // At most 1024 decoded bytes.

#[derive(Debug, Serialize)]
pub(crate) struct PasskeyHint {
    schema: &'static str,
    credential_id: String,
    rp_id: String,
}

// Only selection metadata is decoded. Sessions, public keys and counters are
// ignored, and the identity/key stores are never opened by this command.
#[derive(Deserialize)]
struct Metadata {
    schema: String,
    principals: Vec<Principal>,
}

#[derive(Deserialize)]
struct Principal {
    principal_id: String,
    #[serde(default)]
    display_name: String,
    proof_binding: Binding,
}

#[derive(Deserialize)]
struct Binding {
    kind: String,
    passkey: Option<Passkey>,
}

#[derive(Deserialize)]
struct Passkey {
    credential_id: String,
    rp_id: String,
    revoked_at: Option<u64>,
}

pub(crate) fn read_passkey_hint(
    data_dir: &Path,
    account: Option<&str>,
    principal: Option<&str>,
    rp_id: Option<&str>,
) -> anyhow::Result<PasskeyHint> {
    let path = elastos_server::auth::auth_state_path(data_dir)?;
    // Keep the reader within the auth metadata tree, including on hosts where
    // a local operator has replaced a path component with a symbolic link.
    let relative = path.strip_prefix(data_dir)?;
    let mut component_path = data_dir.to_path_buf();
    for component in relative.components() {
        component_path.push(component);
        let metadata = std::fs::symlink_metadata(&component_path)
            .map_err(|_| anyhow!("existing authentication metadata is unavailable"))?;
        if metadata.file_type().is_symlink() {
            bail!("authentication metadata path requires regular files and directories");
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file: File = options
        .open(path)
        .map_err(|_| anyhow!("existing authentication metadata is unavailable"))?;
    let info = file.metadata()?;
    if !info.is_file() || info.len() > MAX_METADATA_BYTES {
        bail!("authentication metadata exceeds the reader limit or is not a regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        bail!("authentication metadata exceeds the reader limit");
    }
    let metadata: Metadata = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("authentication metadata is invalid"))?;
    if metadata.schema != "elastos.auth.state/v1" {
        bail!("unsupported authentication metadata schema");
    }
    let mut matches = metadata.principals.into_iter().filter_map(|record| {
        if account.is_some_and(|value| value != record.display_name)
            || principal.is_some_and(|value| value != record.principal_id)
            || record.proof_binding.kind != "passkey_web_authn"
        {
            return None;
        }
        record.proof_binding.passkey.filter(|binding| {
            binding.revoked_at.is_none() && rp_id.is_none_or(|value| value == binding.rp_id)
        })
    });
    let selected = matches
        .next()
        .ok_or_else(|| anyhow!("selection has no active passkey binding"))?;
    if matches.next().is_some() {
        bail!("selection is ambiguous; use exact --account, --principal or --rp-id selectors");
    }
    if selected.credential_id.len() > MAX_CREDENTIAL_CHARS {
        bail!("selected passkey metadata is invalid");
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(&selected.credential_id)
        .map_err(|_| anyhow!("selected passkey metadata is invalid"))?;
    if decoded.is_empty()
        || decoded.len() > 1024
        || URL_SAFE_NO_PAD.encode(decoded) != selected.credential_id
        || !valid_rp_id(&selected.rp_id)
    {
        bail!("selected passkey metadata is invalid");
    }
    Ok(PasskeyHint {
        schema: "elastos.passkey.hint/v1",
        credential_id: selected.credential_id,
        rp_id: selected.rp_id,
    })
}

fn valid_rp_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding(name: &str, id: &str, revoked: bool) -> serde_json::Value {
        json!({"principal_id": format!("principal:{name}"), "display_name": name,
            "proof_binding": {"kind": "passkey_web_authn", "passkey": {
                "credential_id": id, "rp_id": "localhost",
                "revoked_at": revoked.then_some(1), "public_key": "ignored", "sign_count": 99}}})
    }

    fn fixture(records: Vec<serde_json::Value>) -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = elastos_server::auth::auth_state_path(root.path()).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({"schema": "elastos.auth.state/v1", "principals": records,
            "sessions": [{"token": "ignored"}]})
            .to_string(),
        )
        .unwrap();
        (root, path)
    }

    #[test]
    fn passkey_hint_reads_one_active_binding_without_mutation_or_extra_output() {
        let (root, path) = fixture(vec![
            binding("Owner", "AQID", false),
            binding("Revoked", "BAUG", true),
        ]);
        let before = std::fs::read(&path).unwrap();
        let hint = read_passkey_hint(root.path(), None, None, None).unwrap();
        assert_eq!(
            serde_json::to_value(hint).unwrap(),
            json!({"schema": "elastos.passkey.hint/v1", "credential_id": "AQID", "rp_id": "localhost"})
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(!root.path().join("identity").exists());
        assert_eq!(
            std::fs::read_dir(root.path().join("ElastOS/System/Auth"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn passkey_hint_requires_exact_unambiguous_active_selection() {
        let (root, path) = fixture(vec![
            binding("Owner", "AQID", false),
            binding("Guest", "BAUG", false),
            binding("Revoked", "BwgJ", true),
        ]);
        let before = std::fs::read(&path).unwrap();
        for (account, principal, rp) in [
            (None, None, None),
            (Some("owner"), None, None),
            (Some("Revoked"), None, None),
            (None, None, Some("wrong.example")),
            (Some("Owner"), Some("principal:Guest"), None),
        ] {
            let error = read_passkey_hint(root.path(), account, principal, rp)
                .unwrap_err()
                .to_string();
            for id in ["AQID", "BAUG", "BwgJ"] {
                assert!(!error.contains(id));
            }
        }
        assert_eq!(
            read_passkey_hint(root.path(), Some("Owner"), None, None)
                .unwrap()
                .credential_id,
            "AQID"
        );
        assert_eq!(
            read_passkey_hint(
                root.path(),
                None,
                Some("principal:Guest"),
                Some("localhost")
            )
            .unwrap()
            .credential_id,
            "BAUG"
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn passkey_hint_refuses_missing_wrong_schema_and_malformed_metadata() {
        let empty = tempfile::tempdir().unwrap();
        assert!(read_passkey_hint(empty.path(), None, None, None).is_err());
        assert_eq!(std::fs::read_dir(empty.path()).unwrap().count(), 0);
        for content in ["{}".to_string(), json!({"schema":"wrong", "principals":[]}).to_string(), json!({"schema":"elastos.auth.state/v1", "principals":[binding("Owner", "sensitive=", false)]}).to_string()] {
            let (root, path) = fixture(vec![]);
            std::fs::write(&path, &content).unwrap();
            let error = read_passkey_hint(root.path(), None, None, None).unwrap_err().to_string();
            assert!(!error.contains("sensitive"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), content);
        }
    }

    #[test]
    fn passkey_hint_refuses_directory_metadata_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let path = elastos_server::auth::auth_state_path(root.path()).unwrap();
        std::fs::create_dir_all(&path).unwrap();
        assert!(read_passkey_hint(root.path(), None, None, None).is_err());
        assert!(path.is_dir());
        assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn passkey_hint_refuses_symbolic_links_to_other_files() {
        let (root, path) = fixture(vec![binding("Owner", "AQID", false)]);
        let target = root.path().join("other-file.json");
        std::fs::rename(&path, &target).unwrap();
        let before = std::fs::read(&target).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read_passkey_hint(root.path(), None, None, None).is_err());
        assert_eq!(std::fs::read(target).unwrap(), before);
        assert!(std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
