use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use elastos_common::CapsuleManifest;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(unix)]
pub(in crate::api) mod preparation;

const DEV_CAPSULES_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../capsules");
const MODEL_CATALOG_FILE: &str = "model-catalog.json";
const MODEL_CATALOG_DOMAIN: &str = "elastos.model.catalog.v1";
const MAX_MODEL_CATALOG_BYTES: usize = 128 * 1024;
const MAX_MODEL_CATALOG_ENTRIES: usize = 8;

pub(crate) struct VerifiedModelCatalogEntry {
    pub cid: String,
    pub publisher_did: String,
    pub manifest: CapsuleManifest,
    pub size_bytes: u64,
    pub object_manifest: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedModelCatalog {
    payload: ModelCatalogPayload,
    signature: String,
    signer_did: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelCatalogPayload {
    schema: String,
    published_at: u64,
    /// `None` is a permanent publisher statement for the pinned snapshot.
    expires_at: Option<u64>,
    entries: Vec<ModelCatalogEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelCatalogEntry {
    cid: String,
    capsule_manifest: Value,
    object_manifest: Value,
}

fn read_model_catalog_file(data_dir: &Path, name: &str, limit: usize) -> anyhow::Result<Vec<u8>> {
    let root = std::fs::symlink_metadata(data_dir)?;
    if !root.is_dir() || root.file_type().is_symlink() {
        anyhow::bail!("model catalog requires a real data directory");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let path = data_dir.join(name);
    let link_metadata = std::fs::symlink_metadata(&path)?;
    if !link_metadata.is_file() || link_metadata.file_type().is_symlink() {
        anyhow::bail!("model catalog input must be a regular file");
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        anyhow::bail!("model catalog input exceeds its file bound");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        if root.uid() != uid
            || root.mode() & 0o022 != 0
            || metadata.uid() != uid
            || metadata.mode() & 0o022 != 0
            || metadata.nlink() != 1
        {
            anyhow::bail!(
                "model catalog input must remain operator-owned and non-writable by others"
            );
        }
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        anyhow::bail!("model catalog input exceeds its byte bound");
    }
    Ok(bytes)
}

pub(crate) fn model_catalog_entries(
    data_dir: &Path,
) -> anyhow::Result<Option<Vec<VerifiedModelCatalogEntry>>> {
    let config: crate::setup::ComponentsManifest = serde_json::from_slice(
        &read_model_catalog_file(data_dir, "components.json", 4 * 1024 * 1024)?,
    )?;
    let Some(trust) = config.model_catalog else {
        return Ok(None);
    };
    let bytes = read_model_catalog_file(data_dir, MODEL_CATALOG_FILE, MAX_MODEL_CATALOG_BYTES)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    verify_model_catalog(&trust, &bytes, now).map(Some)
}

fn verify_model_catalog(
    trust: &crate::setup::ModelCatalogConfig,
    bytes: &[u8],
    now: u64,
) -> anyhow::Result<Vec<VerifiedModelCatalogEntry>> {
    // The shared verifier treats an empty expected set as unrestricted trust.
    if trust.publisher_dids.is_empty()
        || trust.publisher_dids.len() > 8
        || trust
            .publisher_dids
            .iter()
            .any(|did| did.is_empty() || did.len() > 128)
        || trust.publisher_dids.iter().collect::<BTreeSet<_>>().len() != trust.publisher_dids.len()
        || trust.head_cid.len() > 128
        || bytes.len() > MAX_MODEL_CATALOG_BYTES
    {
        anyhow::bail!("model catalog trust or byte bound is invalid");
    }
    let head = cid::Cid::try_from(trust.head_cid.as_str())?;
    if head.version() != cid::Version::V1
        || head.to_string() != trust.head_cid
        || head.codec() != 0x55
        || head.hash().code() != 0x12
        || head.hash().digest() != &Sha256::digest(bytes)[..]
    {
        anyhow::bail!("model catalog head must match the pinned raw SHA-256 CIDv1");
    }
    let signed: SignedModelCatalog = serde_json::from_slice(bytes)?;
    for publisher in &trust.publisher_dids {
        crate::crypto::decode_did_key(publisher)?;
    }
    if signed.signature.len() != 128 || signed.signer_did.len() > 128 {
        anyhow::bail!("model catalog signature fields exceed their bounds");
    }
    crate::crypto::verify_signed_json_envelope_against_dids(
        bytes,
        MODEL_CATALOG_DOMAIN,
        &trust.publisher_dids,
    )?;
    let payload = signed.payload;
    if payload.schema != "elastos.model.catalog/v1"
        || payload.published_at > now
        || payload
            .expires_at
            .is_some_and(|expires| expires <= now || expires <= payload.published_at)
        || !(1..=MAX_MODEL_CATALOG_ENTRIES).contains(&payload.entries.len())
    {
        anyhow::bail!("model catalog requires 1 to 8 unique current signed entries");
    }
    let mut verified = Vec::with_capacity(payload.entries.len());
    let mut seen_cids = BTreeSet::new();
    let mut seen_names = BTreeSet::new();
    for entry in payload.entries {
        if entry.cid.len() > 128 {
            anyhow::bail!("model package CID exceeds its bound");
        }
        let cid = cid::Cid::try_from(entry.cid.as_str())?;
        if cid.version() != cid::Version::V1
            || cid.to_string() != entry.cid
            || cid.codec() != 0x70
            || cid.hash().code() != 0x12
            || cid.hash().digest().len() != 32
        {
            anyhow::bail!("model package requires a canonical DAG-PB SHA-256 closure CIDv1");
        }
        if !seen_cids.insert(entry.cid.clone()) {
            anyhow::bail!("model catalog entries require unique package CIDs");
        }
        if entry
            .object_manifest
            .get("publisher_did")
            .is_some_and(|publisher| {
                !publisher.is_null() && publisher.as_str() != Some(signed.signer_did.as_str())
            })
        {
            anyhow::bail!("model closure publisher must match its configured catalog signer");
        }
        let (manifest, size_bytes) = crate::content::validate_model_content_closure_metadata(
            &entry.capsule_manifest,
            &entry.object_manifest,
        )?;
        if !seen_names.insert(manifest.name.clone()) {
            anyhow::bail!("model catalog entries require unique capsule names");
        }
        verified.push(VerifiedModelCatalogEntry {
            cid: entry.cid,
            publisher_did: signed.signer_did.clone(),
            manifest,
            size_bytes,
            object_manifest: entry.object_manifest,
        });
    }
    Ok(verified)
}

pub(crate) fn installed_capsules_root(data_dir: &Path) -> PathBuf {
    data_dir.join("capsules")
}

pub(crate) fn development_capsules_root() -> PathBuf {
    PathBuf::from(DEV_CAPSULES_ROOT)
}

fn components_manifest(data_dir: &Path) -> Option<crate::setup::ComponentsManifest> {
    let bytes = std::fs::read(data_dir.join("components.json")).ok()?;
    let mut value: Value = serde_json::from_slice(&bytes).ok()?;
    // Model trust errors close only the model catalog. Ordinary installed
    // inventory does not consume that separate optional configuration field.
    value.as_object_mut()?.remove("model_catalog");
    serde_json::from_value(value).ok()
}

pub(crate) fn active_component_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let manifest = components_manifest(data_dir)?;
    Some(installed_external_component_names_from_manifest(
        data_dir, &manifest,
    ))
}

pub(crate) fn active_capsule_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let manifest = components_manifest(data_dir)?;
    Some(
        installed_external_component_names_from_manifest(data_dir, &manifest)
            .into_iter()
            .filter(|name| {
                load_capsule_manifest(&installed_capsules_root(data_dir).join(name), name).is_some()
            })
            .collect(),
    )
}

pub(crate) fn installed_external_component_names(data_dir: &Path) -> Option<BTreeSet<String>> {
    let manifest = components_manifest(data_dir)?;
    Some(installed_external_component_names_from_manifest(
        data_dir, &manifest,
    ))
}

fn installed_external_component_names_from_manifest(
    data_dir: &Path,
    manifest: &crate::setup::ComponentsManifest,
) -> BTreeSet<String> {
    let platform = crate::setup::detect_platform();
    manifest
        .external
        .iter()
        .filter_map(|(name, component)| {
            let platform_info = crate::setup::resolve_platform_info(component, &platform);
            let install_path = crate::setup::resolve_install_path(component, platform_info)?;
            let install_path = Path::new(install_path);
            if install_path.is_absolute()
                || install_path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                || !data_dir.join(install_path).exists()
            {
                return None;
            }
            Some(name.clone())
        })
        .collect()
}

pub(crate) fn installed_active_capsule_dir(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let active_capsules = active_capsule_names(data_dir)?;
    if !active_capsules.contains(name) {
        return None;
    }
    let dir = installed_capsules_root(data_dir).join(name);
    load_capsule_manifest(&dir, name).map(|_| dir)
}

pub(crate) fn load_capsule_manifest(dir: &Path, expected_name: &str) -> Option<CapsuleManifest> {
    if !dir.is_dir() {
        return None;
    }

    let manifest_path = dir.join("capsule.json");
    let bytes = std::fs::read(&manifest_path).ok()?;
    let manifest: CapsuleManifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.validate().is_err() || manifest.name != expected_name {
        return None;
    }
    Some(manifest)
}

fn list_manifests(root: &Path, allowed_names: Option<&BTreeSet<String>>) -> Vec<CapsuleManifest> {
    let mut capsules = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if allowed_names.is_some_and(|allowed| !allowed.contains(name)) {
            continue;
        }
        let Some(manifest) = load_capsule_manifest(&dir, name) else {
            continue;
        };
        capsules.insert(manifest.name.clone(), manifest);
    }
    capsules.into_values().collect()
}

pub(crate) fn list_active_capsule_manifests(data_dir: &Path) -> Vec<CapsuleManifest> {
    let Some(active_capsules) = active_capsule_names(data_dir) else {
        return Vec::new();
    };
    list_manifests(&installed_capsules_root(data_dir), Some(&active_capsules))
}

#[cfg(test)]
pub(crate) fn list_development_capsule_manifests() -> Vec<CapsuleManifest> {
    list_manifests(&development_capsules_root(), None)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn model_catalog_fixture() -> Value {
        // Synthetic signed metadata; no model bytes or production publisher.
        let capsule = serde_json::json!({
            "schema": "elastos.capsule/v1", "version": "0.1.0", "name": "model-fixture",
            "role": "content", "type": "data", "entrypoint": "weights.gguf",
            "projections": ["content"],
            "model_content": {
                "format": "gguf", "quantization": "Q4_K_M", "engine": "llama.cpp",
                "consumer_interface": "elastos.provider.model", "consumer_interface_version": "0.1.0",
                "minimum_memory_mb": 8192,
                "license": {"spdx_id": "Apache-2.0", "path": "LICENSE"},
                "provenance": {
                    "base_repository": "fixture/base", "base_revision": "a".repeat(40),
                    "base_license": {"spdx_id": "Apache-2.0", "path": "LICENSE.base"},
                    "quantized_repository": "fixture/quantized", "quantized_revision": "b".repeat(40),
                    "path": "PROVENANCE.md"
                }
            }
        });
        let capsule_bytes = serde_json::to_vec(&capsule).unwrap();
        let files = [
            ("LICENSE", b"fixture license".as_slice()),
            ("LICENSE.base", b"fixture base license".as_slice()),
            ("PROVENANCE.md", b"fixture provenance".as_slice()),
            ("capsule.json", capsule_bytes.as_slice()),
            ("weights.gguf", b"GGUF fixture metadata only".as_slice()),
        ]
        .into_iter()
        .map(|(path, bytes)| {
            serde_json::json!({
                "path": path, "size": bytes.len(), "sha256": format!("{:x}", Sha256::digest(bytes))
            })
        })
        .collect::<Vec<_>>();
        let mut digest = Sha256::new();
        for file in &files {
            digest.update(file["path"].as_str().unwrap().as_bytes());
            digest.update(b"\0");
            digest.update(file["sha256"].as_str().unwrap().as_bytes());
            digest.update(b"\0");
            digest.update(file["size"].to_string().as_bytes());
            digest.update(b"\0");
        }
        serde_json::json!({
            "schema": "elastos.model.catalog/v1", "published_at": 1, "expires_at": 4_000_000_000_u64,
            "entries": [{
                "cid": "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
                "capsule_manifest": capsule,
                "object_manifest": {
                    "schema": "elastos.content.object.manifest/v1", "kind": "capsule",
                    "content_digest": format!("sha256:{:x}", digest.finalize()), "files": files
                }
            }]
        })
    }

    fn head_cid(bytes: &[u8]) -> String {
        let hash = cid::multihash::Multihash::<64>::wrap(0x12, &Sha256::digest(bytes)).unwrap();
        cid::Cid::new_v1(0x55, hash).to_string()
    }

    fn refresh_catalog_entry_object_manifest(entry: &mut Value) {
        let capsule_bytes = serde_json::to_vec(&entry["capsule_manifest"]).unwrap();
        let files = entry["object_manifest"]["files"].as_array_mut().unwrap();
        for file in files.iter_mut() {
            if file["path"] == "capsule.json" {
                file["size"] = serde_json::json!(capsule_bytes.len() as u64);
                file["sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&capsule_bytes)));
            }
        }
        let mut digest = Sha256::new();
        for file in files {
            digest.update(file["path"].as_str().unwrap().as_bytes());
            digest.update(b"\0");
            digest.update(file["sha256"].as_str().unwrap().as_bytes());
            digest.update(b"\0");
            digest.update(file["size"].to_string().as_bytes());
            digest.update(b"\0");
        }
        entry["object_manifest"]["content_digest"] =
            serde_json::json!(format!("sha256:{:x}", digest.finalize()));
    }

    fn sign_model_catalog(payload: &Value) -> (crate::setup::ModelCatalogConfig, Vec<u8>) {
        let key = elastos_runtime::signature::SigningKey::from_bytes(&[7; 32]);
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            &key,
            MODEL_CATALOG_DOMAIN,
            &serde_json::to_vec(payload).unwrap(),
        );
        let bytes = serde_json::to_vec(&serde_json::json!({
            "payload": payload, "signature": signature, "signer_did": signer_did
        }))
        .unwrap();
        (
            crate::setup::ModelCatalogConfig {
                head_cid: head_cid(&bytes),
                publisher_dids: vec![signer_did],
                local_use: None,
            },
            bytes,
        )
    }

    pub(crate) fn write_model_catalog_fixture(data_dir: &Path, payload: &Value) {
        let (trust, bytes) = sign_model_catalog(payload);
        std::fs::write(
            data_dir.join("components.json"),
            serde_json::to_vec(&serde_json::json!({
                "external": {}, "capsules": {}, "profiles": {}, "model_catalog": trust
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(data_dir.join(MODEL_CATALOG_FILE), bytes).unwrap();
    }

    #[test]
    fn model_catalog_accepts_exact_pinned_signed_metadata() {
        let root = tempfile::tempdir().unwrap();
        write_model_catalog_fixture(root.path(), &model_catalog_fixture());
        let entries = model_catalog_entries(root.path()).unwrap().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].manifest.name, "model-fixture");
        assert!(entries[0].size_bytes > 0);
        assert!(!root.path().join("capsules/model-fixture").exists());
    }

    #[test]
    fn model_catalog_accepts_two_distinct_entries_and_rejects_duplicate_identity() {
        let mut payload = model_catalog_fixture();
        let mut second = payload["entries"][0].clone();
        second["cid"] =
            serde_json::json!("bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm");
        second["capsule_manifest"]["name"] = "model-fixture-b".into();
        refresh_catalog_entry_object_manifest(&mut second);
        payload["entries"].as_array_mut().unwrap().push(second);
        let (trust, bytes) = sign_model_catalog(&payload);
        let entries = verify_model_catalog(&trust, &bytes, 2).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].manifest.name, "model-fixture");
        assert_eq!(entries[1].manifest.name, "model-fixture-b");

        let mut duplicate_cid = model_catalog_fixture();
        let entry = duplicate_cid["entries"][0].clone();
        duplicate_cid["entries"].as_array_mut().unwrap().push(entry);
        let (trust, bytes) = sign_model_catalog(&duplicate_cid);
        assert!(verify_model_catalog(&trust, &bytes, 2).is_err());

        let mut duplicate_name = model_catalog_fixture();
        let mut second = duplicate_name["entries"][0].clone();
        second["cid"] =
            serde_json::json!("bafybeihgnsjhpoktqbyspaqv6moblyny3txs5nkjdxfx7wm346odxkhlrm");
        duplicate_name["entries"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let (trust, bytes) = sign_model_catalog(&duplicate_name);
        assert!(verify_model_catalog(&trust, &bytes, 2).is_err());

        let mut empty = model_catalog_fixture();
        empty["entries"] = serde_json::json!([]);
        let (trust, bytes) = sign_model_catalog(&empty);
        assert!(verify_model_catalog(&trust, &bytes, 2).is_err());
    }

    #[test]
    fn model_catalog_rejects_empty_untrusted_publishers_and_wrong_head() {
        let (trust, bytes) = sign_model_catalog(&model_catalog_fixture());
        for publishers in [
            Vec::new(),
            vec!["did:key:untrusted".into()],
            vec![trust.publisher_dids[0].clone(); 2],
            vec!["x".repeat(129)],
        ] {
            let mut changed = trust.clone();
            changed.publisher_dids = publishers;
            assert!(verify_model_catalog(&changed, &bytes, 2).is_err());
        }
        let mut changed = trust.clone();
        changed.head_cid = head_cid(b"different");
        assert!(verify_model_catalog(&changed, &bytes, 2).is_err());
        let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
        envelope["payload"]["entries"][0]["capsule_manifest"]["name"] = "tampered".into();
        let tampered = serde_json::to_vec(&envelope).unwrap();
        changed.head_cid = head_cid(&tampered);
        assert!(
            verify_model_catalog(&changed, &tampered, 2).is_err(),
            "even a repinned payload needs the configured signature"
        );
        assert!(verify_model_catalog(&trust, &vec![0; MAX_MODEL_CATALOG_BYTES + 1], 2).is_err());
    }

    #[test]
    fn model_catalog_rejects_invalid_or_incomplete_closures() {
        for (pointer, value) in [
            ("/schema", serde_json::json!("unknown")),
            ("/published_at", serde_json::json!(3)),
            ("/expires_at", serde_json::json!(2)),
            (
                "/entries/0/cid",
                serde_json::json!(head_cid(b"raw is not a directory")),
            ),
            (
                "/entries/0/object_manifest/content_digest",
                serde_json::json!("sha256:wrong"),
            ),
            (
                "/entries/0/object_manifest/files/0/path",
                serde_json::json!("../LICENSE"),
            ),
            (
                "/entries/0/object_manifest/files/1/path",
                serde_json::json!("license"),
            ),
            (
                "/entries/0/object_manifest/files/0/size",
                serde_json::json!(0),
            ),
            (
                "/entries/0/object_manifest/files/4/size",
                serde_json::json!(17_u64 * 1024 * 1024 * 1024),
            ),
            (
                "/entries/0/object_manifest/files/3/sha256",
                serde_json::json!("0".repeat(64)),
            ),
            (
                "/entries/0/capsule_manifest/model_content/provenance/path",
                serde_json::json!("missing.md"),
            ),
        ] {
            let mut payload = model_catalog_fixture();
            *payload.pointer_mut(pointer).unwrap() = value;
            let (trust, bytes) = sign_model_catalog(&payload);
            assert!(
                verify_model_catalog(&trust, &bytes, 2).is_err(),
                "{pointer}"
            );
        }
        for pointer in [
            "",
            "/entries/0",
            "/entries/0/object_manifest",
            "/entries/0/object_manifest/files/0",
        ] {
            let mut payload = model_catalog_fixture();
            payload.pointer_mut(pointer).unwrap()["extra"] = true.into();
            let (trust, bytes) = sign_model_catalog(&payload);
            assert!(
                verify_model_catalog(&trust, &bytes, 2).is_err(),
                "extra field at {pointer}"
            );
        }
        let mut payload = model_catalog_fixture();
        let entry = payload["entries"][0].clone();
        payload["entries"].as_array_mut().unwrap().push(entry);
        let (trust, bytes) = sign_model_catalog(&payload);
        assert!(verify_model_catalog(&trust, &bytes, 2).is_err());
    }

    fn permanent_model_catalog_fixture() -> Value {
        let mut payload = model_catalog_fixture();
        payload.as_object_mut().unwrap().remove("expires_at");
        payload
    }

    #[test]
    fn model_catalog_permanent_snapshot_verifies_at_any_later_time() {
        let (trust, bytes) = sign_model_catalog(&permanent_model_catalog_fixture());
        for now in [1, 2, 4_000_000_001, u64::MAX] {
            let entries = verify_model_catalog(&trust, &bytes, now).unwrap();
            assert_eq!(entries.len(), 1, "now={now}");
            assert_eq!(entries[0].manifest.name, "model-fixture", "now={now}");
        }
        assert!(
            verify_model_catalog(&trust, &bytes, 0).is_err(),
            "publication in the future stays rejected"
        );
    }

    #[test]
    fn model_catalog_null_expires_at_decodes_as_permanent() {
        let mut payload = model_catalog_fixture();
        payload["expires_at"] = Value::Null;
        let (trust, bytes) = sign_model_catalog(&payload);
        let signed: SignedModelCatalog = serde_json::from_slice(&bytes).unwrap();
        assert!(signed.payload.expires_at.is_none());
        let entries = verify_model_catalog(&trust, &bytes, u64::MAX).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].manifest.name, "model-fixture");
    }

    #[test]
    fn model_catalog_timed_snapshot_rejects_exactly_at_expiry() {
        let mut payload = model_catalog_fixture();
        payload["expires_at"] = serde_json::json!(10);
        let (trust, bytes) = sign_model_catalog(&payload);
        let entries = verify_model_catalog(&trust, &bytes, 9).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].manifest.name, "model-fixture");
        assert!(verify_model_catalog(&trust, &bytes, 10).is_err());
        assert!(verify_model_catalog(&trust, &bytes, 11).is_err());
        for expires_at in [1, 0] {
            let mut payload = model_catalog_fixture();
            payload["expires_at"] = serde_json::json!(expires_at);
            let (trust, bytes) = sign_model_catalog(&payload);
            assert!(
                verify_model_catalog(&trust, &bytes, 1).is_err(),
                "expires_at={expires_at} never postdates published_at=1"
            );
        }
    }

    #[test]
    fn model_catalog_permanent_snapshot_keeps_pin_signature_and_publisher_checks() {
        let (trust, bytes) = sign_model_catalog(&permanent_model_catalog_fixture());
        let entries = verify_model_catalog(&trust, &bytes, u64::MAX).unwrap();
        assert_eq!(entries[0].manifest.name, "model-fixture");
        let untrusted = "did:key:untrusted";
        let mut changed = trust.clone();
        changed.head_cid = head_cid(b"different");
        assert!(verify_model_catalog(&changed, &bytes, u64::MAX).is_err());
        for publishers in [vec![untrusted.to_owned()], Vec::new()] {
            let mut changed = trust.clone();
            changed.publisher_dids = publishers;
            assert!(verify_model_catalog(&changed, &bytes, u64::MAX).is_err());
        }
        let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
        envelope["payload"]["entries"][0]["capsule_manifest"]["name"] = "tampered".into();
        let tampered = serde_json::to_vec(&envelope).unwrap();
        let mut repinned = trust.clone();
        repinned.head_cid = head_cid(&tampered);
        assert!(
            verify_model_catalog(&repinned, &tampered, u64::MAX).is_err(),
            "a repinned permanent payload still needs the configured signature"
        );
        let mut envelope: Value = serde_json::from_slice(&bytes).unwrap();
        envelope["signer_did"] = untrusted.into();
        let reassigned = serde_json::to_vec(&envelope).unwrap();
        repinned.head_cid = head_cid(&reassigned);
        assert!(verify_model_catalog(&repinned, &reassigned, u64::MAX).is_err());
        let mut payload = permanent_model_catalog_fixture();
        payload["renewal"] = true.into();
        let (trust, bytes) = sign_model_catalog(&payload);
        assert!(
            verify_model_catalog(&trust, &bytes, u64::MAX).is_err(),
            "unknown payload fields stay rejected for permanent snapshots"
        );
    }

    #[test]
    fn model_catalog_permanent_snapshot_verifies_after_reread() {
        let root = tempfile::tempdir().unwrap();
        write_model_catalog_fixture(root.path(), &permanent_model_catalog_fixture());
        let first = model_catalog_entries(root.path()).unwrap().unwrap();
        let second = model_catalog_entries(root.path()).unwrap().unwrap();
        for entries in [&first, &second] {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].manifest.name, "model-fixture");
        }
        assert_eq!(first[0].cid, second[0].cid);
    }

    #[cfg(unix)]
    #[test]
    fn model_catalog_rejects_symlink_hardlink_and_writable_snapshot() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        for name in [MODEL_CATALOG_FILE, "components.json"] {
            let root = tempfile::tempdir().unwrap();
            write_model_catalog_fixture(root.path(), &model_catalog_fixture());
            let path = root.path().join(name);
            let original = root.path().join("original");
            std::fs::rename(&path, &original).unwrap();
            symlink(&original, &path).unwrap();
            assert!(model_catalog_entries(root.path()).is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::hard_link(&original, &path).unwrap();
            assert!(model_catalog_entries(root.path()).is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&original, &path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
            assert!(model_catalog_entries(root.path()).is_err());
        }
    }

    #[test]
    fn product_inventory_excludes_source_only_capsules() {
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            data_dir.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "gba-emulator": {
                        "install_path": "capsules/gba-emulator",
                        "platforms": {}
                    },
                    "gba-ucity": {
                        "install_path": "capsules/gba-ucity",
                        "platforms": {}
                    },
                    "binary-only": {
                        "install_path": "bin/binary-only",
                        "platforms": {}
                    }
                },
                "capsules": {},
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
        let binary_capsule = data_dir.path().join("capsules/binary-only");
        std::fs::create_dir_all(&binary_capsule).unwrap();
        std::fs::write(
            binary_capsule.join("capsule.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "elastos.capsule/v1",
                "name": "binary-only",
                "version": "0.1.0",
                "role": "app",
                "type": "data",
                "entrypoint": "index.html"
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(binary_capsule.join("index.html"), "test").unwrap();

        let names = list_active_capsule_manifests(data_dir.path())
            .into_iter()
            .map(|manifest| manifest.name)
            .collect::<BTreeSet<_>>();

        assert!(
            names.is_empty(),
            "source directories are not product installs"
        );
        let development_names = list_development_capsule_manifests()
            .into_iter()
            .map(|manifest| manifest.name)
            .collect::<BTreeSet<_>>();
        assert!(development_names.contains("gba-emulator"));
        assert!(development_names.contains("gba-ucity"));
    }

    #[test]
    fn product_inventory_requires_an_installed_component_and_capsule_contract() {
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            data_dir.path().join("components.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "external": {
                    "object-provider": {
                        "install_path": "bin/object-provider",
                        "platforms": {}
                    },
                    "missing-provider": {
                        "install_path": "bin/missing-provider",
                        "platforms": {}
                    }
                },
                "capsules": {
                    "source-registry-only": {
                        "cid": "bafy-source",
                        "sha256": "",
                        "size": 0
                    }
                },
                "profiles": {}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(data_dir.path().join("bin")).unwrap();
        std::fs::write(data_dir.path().join("bin/object-provider"), "provider").unwrap();
        let contract_dir = data_dir.path().join("capsules/object-provider");
        std::fs::create_dir_all(&contract_dir).unwrap();
        std::fs::copy(
            development_capsules_root().join("object-provider/capsule.json"),
            contract_dir.join("capsule.json"),
        )
        .unwrap();

        assert_eq!(
            active_component_names(data_dir.path()).unwrap(),
            BTreeSet::from(["object-provider".to_string()])
        );
        assert_eq!(
            active_capsule_names(data_dir.path()).unwrap(),
            BTreeSet::from(["object-provider".to_string()])
        );
    }

    #[test]
    fn checkout_permanent_catalog_matches_components_pin() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../");
        let components: crate::setup::ComponentsManifest =
            serde_json::from_slice(&std::fs::read(root.join("components.json")).unwrap()).unwrap();
        let trust = components
            .model_catalog
            .expect("matching install pins a signed model catalog");
        let bytes = std::fs::read(root.join(MODEL_CATALOG_FILE)).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let entries = verify_model_catalog(&trust, &bytes, now).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].cid,
            "bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi"
        );
        assert_eq!(entries[0].manifest.name, "qwen3-5-9b-q4-k-m-local");
        assert_eq!(
            entries[1].cid,
            "bafybeidy5kfvqwg6g6pfgdfwslmhijosbeskt5b2duqdqxnc7e6fwmr72y"
        );
        assert_eq!(entries[1].manifest.name, "smollm2-135m-instruct-q8-0-local");
        assert_eq!(
            entries[0].publisher_did,
            "did:key:z6Mkjg9duxEF2nskEPR9F38eSfrY6F1GyWUq5aDbgsMqgcjA"
        );
        assert_eq!(entries[1].publisher_did, entries[0].publisher_did);
    }
}
