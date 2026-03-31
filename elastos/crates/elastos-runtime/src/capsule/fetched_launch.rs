//! Transitional fetched-capsule launch preparation.
//!
//! Prefer a real fetched capsule bundle (`capsule.json` + entrypoint) when the
//! fetched content already provides one. Raw fetched WASM is no longer accepted
//! on this path.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use elastos_common::CapsuleManifest;
use elastos_namespace::FetchResult;
use flate2::read::GzDecoder;
use tar::Archive;
use tempfile::TempDir;

/// Prepared fetched capsule ready for `CapsuleManager::launch_from_cid`.
///
/// The contained manifest is the bundle-provided manifest. Content-addressed
/// verification may have succeeded, but there is still no fetched
/// signature-based trust model on this path.
#[derive(Debug)]
pub struct PreparedFetchedCapsule {
    _temp_dir: TempDir,
    launch_dir: PathBuf,
    manifest: CapsuleManifest,
}

impl PreparedFetchedCapsule {
    pub fn path(&self) -> &Path {
        &self.launch_dir
    }

    pub fn manifest(&self) -> &CapsuleManifest {
        &self.manifest
    }

    pub fn into_manifest(self) -> CapsuleManifest {
        self.manifest
    }
}

pub fn prepare_fetched_capsule(
    _cid: &str,
    fetch_result: FetchResult,
) -> Result<PreparedFetchedCapsule, String> {
    let temp_dir =
        tempfile::tempdir().map_err(|err| format!("Failed to create temp dir: {}", err))?;
    if let Some((launch_dir, manifest)) =
        try_prepare_bundle(temp_dir.path(), &fetch_result.content)?
    {
        return Ok(PreparedFetchedCapsule {
            _temp_dir: temp_dir,
            launch_dir,
            manifest,
        });
    }

    Err(
        "Fetched content is not a capsule bundle with capsule.json and validated entrypoint. Raw fetched WASM launch is no longer supported on this path."
            .to_string(),
    )
}

fn try_prepare_bundle(
    staging_root: &Path,
    content: &[u8],
) -> Result<Option<(PathBuf, CapsuleManifest)>, String> {
    if !looks_like_gzip(content) {
        return Ok(None);
    }

    let decoder = GzDecoder::new(Cursor::new(content));
    let mut archive = Archive::new(decoder);
    let entries = archive.entries().map_err(|err| {
        format!(
            "Fetched capsule bundle is not a readable tar archive: {}",
            err
        )
    })?;

    for entry in entries {
        let mut entry =
            entry.map_err(|err| format!("Failed to read fetched capsule bundle entry: {}", err))?;
        entry
            .unpack_in(staging_root)
            .map_err(|err| format!("Failed to unpack fetched capsule bundle: {}", err))?;
    }

    let manifest_paths = find_manifest_paths(staging_root)?;
    let manifest_path = match manifest_paths.as_slice() {
        [] => {
            return Err(
                "Fetched capsule bundle is missing capsule.json; fetched launch requires a real bundle manifest."
                    .to_string(),
            )
        }
        [path] => path.clone(),
        _ => {
            return Err(format!(
                "Fetched capsule bundle contains multiple capsule.json files: {}",
                manifest_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        }
    };

    let manifest_dir = manifest_path
        .parent()
        .ok_or_else(|| "Fetched capsule bundle manifest has no parent directory".to_string())?;
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|err| format!("Failed to read fetched capsule manifest: {}", err))?;
    let manifest: CapsuleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| format!("Invalid fetched capsule manifest: {}", err))?;
    manifest
        .validate()
        .map_err(|err| format!("Fetched capsule manifest failed validation: {}", err))?;

    let entrypoint_path = manifest_dir.join(&manifest.entrypoint);
    if !entrypoint_path.is_file() {
        return Err(format!(
            "Fetched capsule bundle entrypoint missing: {}",
            entrypoint_path.display()
        ));
    }

    Ok(Some((manifest_dir.to_path_buf(), manifest)))
}

fn looks_like_gzip(content: &[u8]) -> bool {
    content.starts_with(&[0x1f, 0x8b])
}

fn find_manifest_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    collect_manifest_paths(root, &mut found)?;
    Ok(found)
}

fn collect_manifest_paths(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        format!(
            "Failed to read fetched capsule directory {}: {}",
            dir.display(),
            err
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "Failed to inspect fetched capsule directory {}: {}",
                dir.display(),
                err
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_manifest_paths(&path, found)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("capsule.json") {
            found.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_namespace::FetchSource;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::Builder;

    fn build_bundle(files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = Builder::new(encoder);
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(content.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, path, *content)
                .expect("append bundle entry");
        }
        let encoder = tar.into_inner().expect("finish tar");
        encoder.finish().expect("finish gzip")
    }

    #[test]
    fn prepared_capsule_prefers_real_bundle_manifest() {
        let bundle = build_bundle(&[
            (
                "chat/capsule.json",
                br#"{
                    "schema": "elastos.capsule/v1",
                    "version": "1.2.3",
                    "name": "chat",
                    "type": "wasm",
                    "entrypoint": "chat.wasm"
                }"#,
            ),
            ("chat/chat.wasm", &[0, 97, 115, 109]),
        ]);

        let prepared = prepare_fetched_capsule(
            "QmBundleCid",
            FetchResult {
                content: bundle,
                source: FetchSource::LocalCache,
                verified: true,
            },
        )
        .expect("bundle should prepare");

        assert_eq!(prepared.manifest().name, "chat");
        assert_eq!(prepared.manifest().entrypoint, "chat.wasm");
        assert!(prepared.path().join("chat.wasm").is_file());
        assert!(prepared.path().join("capsule.json").is_file());
    }

    #[test]
    fn gzip_bundle_missing_manifest_fails_closed() {
        let bundle = build_bundle(&[("chat/chat.wasm", &[0, 97, 115, 109])]);

        let err = prepare_fetched_capsule(
            "QmBundleCid",
            FetchResult {
                content: bundle,
                source: FetchSource::LocalCache,
                verified: true,
            },
        )
        .expect_err("bundle without manifest should fail");

        assert!(err.contains("missing capsule.json"));
    }

    #[test]
    fn raw_wasm_fails_closed_without_explicit_compatibility() {
        let err = prepare_fetched_capsule(
            "QmExampleCid",
            FetchResult {
                content: vec![0, 97, 115, 109],
                source: FetchSource::LocalCache,
                verified: true,
            },
        )
        .expect_err("raw wasm should fail closed");

        assert!(err.contains("not a capsule bundle"));
        assert!(err.contains("no longer supported"));
    }
}
