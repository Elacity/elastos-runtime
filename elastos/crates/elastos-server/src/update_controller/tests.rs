use super::*;
use crate::host_lock::test_support::SpawnedWhileOpen;
use elastos_common::localhost::{
    installation_release_head_path, installation_release_manifest_path,
};
use serde_json::{json, Value};
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{symlink, PermissionsExt};

type SnapshotEntry = (u32, Option<Vec<u8>>, Option<PathBuf>);
type Snapshot = std::collections::BTreeMap<PathBuf, SnapshotEntry>;

struct PrivateFixture {
    _root: tempfile::TempDir,
    data: PathBuf,
    directory: PathBuf,
    binary: PathBuf,
}

impl PrivateFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = fs::canonicalize(root.path()).unwrap();
        let directory = controller_directory(&data).unwrap();
        let binary = data.join("installed-runtime");
        let fixture = Self {
            _root: root,
            data,
            directory,
            binary,
        };
        fixture.file(&fixture.binary, b"signed fixture Runtime", 0o755);
        fixture
    }

    fn file(&self, path: &Path, bytes: &[u8], mode: u32) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .unwrap();
        file.set_permissions(fs::Permissions::from_mode(mode))
            .unwrap();
        file.write_all(bytes).unwrap();
    }

    fn controller(&self) -> PathBuf {
        self.directory.join("runtime")
    }

    fn publish_installed_release(&self) -> TrustedSource {
        self.publish_installed_release_with_components(
            br#"{"schema":"elastos.components/v1","external":{},"capsules":{},"profiles":{}}"#,
        )
    }

    fn publish_installed_release_with_components(&self, components: &[u8]) -> TrustedSource {
        self.file(&self.data.join("components.json"), components, 0o600);
        let binary = fs::read(&self.binary).unwrap();
        let descriptor = |bytes: &[u8]| {
            json!({
                "cid": raw_cid(bytes), "sha256": digest(bytes), "size": bytes.len()
            })
        };
        let release = signed(
            json!({
                "schema":"elastos.release/v1", "version":"0.7.0", "channel":"stable",
                "platforms": {(crate::update::detect_release_platform()): {
                    "binary": descriptor(&binary), "components": descriptor(components)
                }}
            }),
            "elastos.release.v1",
        );
        let head = signed(
            json!({
                "schema":"elastos.release.head/v1", "version":"0.7.0", "channel":"stable",
                "latest_release_cid": raw_cid(&release), "release_sha256":digest(&release)
            }),
            "elastos.release.head.v1",
        );
        let mut config = source_config(&self.binary);
        config.sources[0].head_cid = raw_cid(&head);
        // Bootstrap refusal tests snapshot after the same persisted writer lock exists.
        drop(
            crate::install_transaction::InstallationGuard::acquire(self.binary.parent().unwrap())
                .unwrap(),
        );
        crate::sources::save_trusted_sources(&self.data, &config).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(self.data.join("installation"))
            .unwrap();
        self.file(&installation_release_head_path(&self.data), &head, 0o600);
        self.file(
            &installation_release_manifest_path(&self.data),
            &release,
            0o600,
        );
        config.sources.remove(0)
    }

    fn snapshot(&self) -> Snapshot {
        fn collect(root: &Path, path: &Path, result: &mut Snapshot) {
            let metadata = fs::symlink_metadata(path).unwrap();
            let bytes = metadata.is_file().then(|| fs::read(path).unwrap());
            let target = metadata
                .file_type()
                .is_symlink()
                .then(|| fs::read_link(path).unwrap());
            result.insert(
                path.strip_prefix(root).unwrap().into(),
                (metadata.mode(), bytes, target),
            );
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    collect(root, &entry.unwrap().path(), result);
                }
            }
        }
        let mut result = std::collections::BTreeMap::new();
        collect(&self.data, &self.data, &mut result);
        result
    }
}

fn test_key() -> ed25519_dalek::SigningKey {
    // Deterministic disposable fixture identity, separate from all publisher keys.
    ed25519_dalek::SigningKey::from_bytes(&[43; 32])
}

fn test_source() -> TrustedSource {
    serde_json::from_value(json!({
        "name": "restart-fixture",
        "publisher_dids": [crate::crypto::encode_signing_key_did(&test_key())],
        "channel": "stable",
        "installed_version": "0.7.0"
    }))
    .unwrap()
}

fn signed(payload: Value, domain: &str) -> Vec<u8> {
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &test_key(),
        domain,
        &serde_json::to_vec(&payload).unwrap(),
    );
    serde_json::to_vec(
        &json!({"payload": payload, "signature": signature, "signer_did": signer_did}),
    )
    .unwrap()
}

fn raw_cid(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let multihash = cid::multihash::Multihash::<64>::wrap(0x12, hash.as_slice()).unwrap();
    cid::Cid::new_v1(0x55, multihash).to_string()
}

fn signed_release(binary_hash: &str) -> Vec<u8> {
    let mut payload = json!({
        "schema": "elastos.release/v1", "version": "0.7.0", "channel": "stable", "platforms": {}
    });
    payload["platforms"][crate::update::detect_release_platform()] =
        json!({"binary": {"sha256": binary_hash}});
    signed(payload, "elastos.release.v1")
}

fn choice_fixture() -> (TrustedSource, UpdateRequest, Vec<u8>) {
    let source = test_source();
    let release_cid = raw_cid(b"chosen signed release fixture");
    let head = signed(
        json!({
            "schema": "elastos.release.head/v1", "version": "0.7.1", "channel": "stable",
            "latest_release_cid": release_cid
        }),
        "elastos.release.head.v1",
    );
    let choice = UpdateRequest {
        id: "c".repeat(32),
        source_name: source.name.clone(),
        channel: "stable".into(),
        publisher_did: source.publisher_dids[0].clone(),
        current_version: "0.7.0".into(),
        new_version: "0.7.1".into(),
        head_cid: raw_cid(&head),
        release_cid,
    };
    (source, choice, head)
}

fn source_config(binary: &Path) -> crate::sources::TrustedSourcesConfig {
    let mut source = test_source();
    source.install_path = binary.to_str().unwrap().into();
    crate::sources::TrustedSourcesConfig {
        schema: "elastos.trusted-sources/v1".into(),
        default_source: source.name.clone(),
        sources: vec![source],
    }
}

fn publish_retained_receipt(fixture: &PrivateFixture) {
    let expected = digest(&fs::read(&fixture.binary).unwrap());
    copy_controller(&fixture.binary, &fixture.controller(), &expected).unwrap();
    let launch = LaunchPlan {
        args: vec![BASE64.encode(b"home"), BASE64.encode(b"--browser")],
        environment: Vec::new(),
        cwd: BASE64.encode(fixture.data.as_os_str().as_bytes()),
    };
    let receipt = Receipt {
        schema: "elastos.update-controller/v1".into(),
        data_dir: fixture.data.clone(),
        binary: fixture.binary.clone(),
        controller: fixture.controller(),
        controller_sha256: expected.clone(),
        signed_controller_release: BASE64.encode(signed_release(&expected)),
        trusted_source: source_config(&fixture.binary).sources.remove(0),
        launch_sha256: launch.sha256().unwrap(),
        launch,
    };
    write_private(&fixture.directory.join(RECEIPT), &receipt).unwrap();
    validate_retained_receipt(&receipt).unwrap();
}

fn prepare_host_fence_release(
    fixture: &PrivateFixture,
    source_binary: &Path,
    restarting: bool,
) -> InstallTransaction {
    use crate::install_transaction::ReleaseFile;

    let writer = InstallTransaction::acquire(&fixture.data, &fixture.binary).unwrap();
    let sources = serde_json::to_vec(&source_config(source_binary)).unwrap();
    writer
        .prepare(&[
            (ReleaseFile::RuntimeBinary, b"candidate fixture Runtime"),
            (ReleaseFile::Components, b"candidate components"),
            (ReleaseFile::Sources, &sources),
            (ReleaseFile::ReleaseHead, b"candidate release head"),
            (ReleaseFile::ReleaseManifest, b"candidate release manifest"),
        ])
        .unwrap();
    if restarting {
        writer
            .prepare_restart(RestartPlan {
                request_id: "c".repeat(32),
                controller_sha256: digest(b"signed fixture Runtime"),
                launch_plan_sha256: digest(b"fixture launch"),
                support_sha256: digest(b"fixture support"),
                support_paths: Default::default(),
                previous_version: "0.7.0".into(),
                candidate_version: "0.7.1".into(),
                previous_binary_sha256: digest(b"signed fixture Runtime"),
            })
            .unwrap();
    }
    writer
}

fn host_fence_migration_target(fixture: &PrivateFixture) -> (PathBuf, PathBuf, PathBuf) {
    let principal_id = "person:local:host-fence";
    let protection = crate::auth::store_test_principal_root_protection(&fixture.data, principal_id);
    let object_uri = format!(
        "{}/.AppData/LocalHost/GBA/ucity/save.sav",
        protection.localhost_root
    );
    let target =
        elastos_common::localhost::rooted_localhost_fs_path(&fixture.data, &object_uri).unwrap();
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fixture.file(&target, b"owner plaintext save", 0o600);
    let plan = crate::auth::PrincipalRootMigrationPlanV1 {
        schema: crate::auth::PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.into(),
        principal_id: principal_id.into(),
        localhost_root: protection.localhost_root,
        objects: vec![crate::auth::PrincipalRootMigrationSelectionV1 {
            object_uri,
            plaintext_sha256: format!("sha256:{}", digest(b"owner plaintext save")),
        }],
    };
    let plan_path = fixture.data.join("migration-plan.json");
    fixture.file(&plan_path, &serde_json::to_vec(&plan).unwrap(), 0o600);
    (target, plan_path, fixture.data.join("backups/host-fence"))
}

#[test]
fn alternate_binary_refuses_installed_recovery_before_start_or_offline_migration() {
    for retained in [false, true] {
        for restarting in [false, true] {
            let fixture = PrivateFixture::new();
            fixture.publish_installed_release();
            if retained {
                publish_retained_receipt(&fixture);
            }
            let alternate = fixture.data.join("alternate-bin/installed-runtime");
            fs::create_dir(alternate.parent().unwrap()).unwrap();
            fixture.file(&alternate, b"candidate fixture Runtime", 0o755);
            assert_ne!(alternate.parent(), fixture.binary.parent());
            assert_ne!(
                std::env::current_exe().unwrap().parent(),
                fixture.binary.parent()
            );
            // A retained receipt owns recovery even when source settings point elsewhere.
            let source_binary = if retained {
                &alternate
            } else {
                &fixture.binary
            };
            let writer = prepare_host_fence_release(&fixture, source_binary, restarting);
            if restarting {
                writer.commit_checked(|| Ok(())).unwrap();
            }
            let claim = restarting.then(|| writer.claim_start(false).unwrap());
            let (target, plan, backup) = host_fence_migration_target(&fixture);
            let before = fixture.snapshot();
            let expected = if restarting {
                "retained update controller"
            } else {
                "Run `elastos update` again"
            };
            for result in [
                crate::install_transaction::authorize_host_start_with_generation(
                    &fixture.data,
                    &alternate,
                    claim.as_ref().map(|claim| claim.generation.as_str()),
                    std::process::id(),
                ),
                crate::host_lock::acquire_host_process_lock(&fixture.data, "gateway", "offline")
                    .map(|_| ()),
                crate::auth::migrate_principal_root_objects_offline(&fixture.data, &plan, &backup)
                    .map(|_| ()),
                crate::api::auth_gateway::migrate_configured_principal_roots_offline(
                    &fixture.data,
                    &backup,
                )
                .map(|_| ()),
            ] {
                let message = result.unwrap_err().to_string();
                assert!(message.contains(expected), "{message}, retained={retained}");
                assert_eq!(fixture.snapshot(), before);
            }
            assert_eq!(fs::read(target).unwrap(), b"owner plaintext save");
            assert!(!backup.exists());
            if let Some(claim) = claim {
                fixture.file(
                    &alternate
                        .parent()
                        .unwrap()
                        .join(".elastos.update-journal.json"),
                    b"{uncertain invoking journal",
                    0o600,
                );
                let before = fixture.snapshot();
                assert!(
                    crate::install_transaction::authorize_host_start_with_generation(
                        &fixture.data,
                        &alternate,
                        Some(&claim.generation),
                        std::process::id(),
                    )
                    .is_err()
                );
                assert_eq!(fixture.snapshot(), before);
                crate::install_transaction::authorize_host_start_with_generation(
                    &fixture.data,
                    &fixture.binary,
                    Some(&claim.generation),
                    std::process::id(),
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn ordinary_start_and_offline_migration_keep_fresh_or_installed_homes_available() {
    for installed in [false, true] {
        let fixture = PrivateFixture::new();
        if installed {
            fixture.publish_installed_release();
            publish_retained_receipt(&fixture);
        }
        let (target, plan, backup) = host_fence_migration_target(&fixture);
        let host = crate::host_lock::acquire_host_process_lock(&fixture.data, "gateway", "offline")
            .unwrap();
        drop(host);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(backup.parent().unwrap())
            .unwrap();
        let receipt =
            crate::auth::migrate_principal_root_objects_offline(&fixture.data, &plan, &backup)
                .unwrap();
        assert_eq!(receipt.object_count, 1);
        assert_ne!(fs::read(target).unwrap(), b"owner plaintext save");
        assert!(backup.exists());
    }
}

#[test]
fn alternate_binary_refuses_foreign_or_uncertain_installed_journals_without_writes() {
    for retained in [false, true] {
        for state in [
            "foreign home",
            "foreign binary",
            "malformed",
            "unsafe",
            "symlink",
            "directory",
            "absent sources",
        ] {
            if state == "absent sources" && !retained {
                continue;
            }
            let fixture = PrivateFixture::new();
            fixture.publish_installed_release();
            if retained {
                publish_retained_receipt(&fixture);
            }
            let writer = prepare_host_fence_release(&fixture, &fixture.binary, false);
            let journal = fixture
                .binary
                .parent()
                .unwrap()
                .join(".elastos.update-journal.json");
            match state {
                "foreign home" | "foreign binary" => {
                    let mut value: Value =
                        serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
                    if state == "foreign home" {
                        value["data_dir"] = json!(fixture.data.join("another-home"));
                    } else {
                        value["binary_basename"] = json!("foreign-runtime");
                    }
                    fs::write(&journal, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "malformed" => fs::write(&journal, b"{malformed").unwrap(),
                "unsafe" => {
                    fs::set_permissions(&journal, fs::Permissions::from_mode(0o666)).unwrap()
                }
                "symlink" => {
                    fs::remove_file(&journal).unwrap();
                    symlink(&fixture.binary, &journal).unwrap();
                }
                "directory" => {
                    fs::remove_file(&journal).unwrap();
                    fs::create_dir(&journal).unwrap();
                }
                "absent sources" => fs::remove_file(fixture.data.join("sources.json")).unwrap(),
                _ => unreachable!(),
            }
            let (target, plan, backup) = host_fence_migration_target(&fixture);
            let before = fixture.snapshot();
            for result in [
                crate::host_lock::acquire_host_process_lock(&fixture.data, "gateway", "offline")
                    .map(|_| ()),
                crate::auth::migrate_principal_root_objects_offline(&fixture.data, &plan, &backup)
                    .map(|_| ()),
                crate::api::auth_gateway::migrate_configured_principal_roots_offline(
                    &fixture.data,
                    &backup,
                )
                .map(|_| ()),
            ] {
                assert!(result.is_err(), "{state}, retained={retained}");
                assert_eq!(fixture.snapshot(), before, "{state}, retained={retained}");
            }
            assert_eq!(fs::read(target).unwrap(), b"owner plaintext save");
            assert!(!backup.exists());
            drop(writer);
        }
    }
}

#[test]
fn invoking_journal_refuses_when_the_trusted_installation_parent_is_clean() {
    for malformed in [false, true] {
        let fixture = PrivateFixture::new();
        fixture.publish_installed_release();
        let alternate = fixture.data.join("alternate-bin/installed-runtime");
        fs::create_dir(alternate.parent().unwrap()).unwrap();
        fixture.file(&alternate, b"signed fixture Runtime", 0o755);
        let writer = prepare_host_fence_release(&fixture, &fixture.binary, false);
        let installed_journal = fixture
            .binary
            .parent()
            .unwrap()
            .join(".elastos.update-journal.json");
        let bytes = fs::read(&installed_journal).unwrap();
        writer.recover().unwrap();
        assert!(!installed_journal.exists());
        fixture.file(
            &alternate
                .parent()
                .unwrap()
                .join(".elastos.update-journal.json"),
            if malformed {
                b"{uncertain invoking journal"
            } else {
                &bytes
            },
            0o600,
        );
        let before = fixture.snapshot();
        assert!(
            crate::install_transaction::authorize_host_start_with_generation(
                &fixture.data,
                &alternate,
                None,
                std::process::id(),
            )
            .is_err()
        );
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn ordinary_admission_refuses_invalid_installation_authority_without_writes() {
    for state in [
        "source json",
        "relative source",
        "receipt json",
        "missing controller",
    ] {
        let fixture = PrivateFixture::new();
        fixture.publish_installed_release();
        if matches!(state, "receipt json" | "missing controller") {
            publish_retained_receipt(&fixture);
        }
        let (_, plan, backup) = host_fence_migration_target(&fixture);
        match state {
            "source json" => {
                fs::write(fixture.data.join("sources.json"), b"{invalid source").unwrap()
            }
            "relative source" => fs::write(
                fixture.data.join("sources.json"),
                serde_json::to_vec(&source_config(Path::new("relative/runtime"))).unwrap(),
            )
            .unwrap(),
            "receipt json" => {
                fs::write(fixture.directory.join(RECEIPT), b"{invalid receipt").unwrap()
            }
            "missing controller" => fs::remove_file(fixture.controller()).unwrap(),
            _ => unreachable!(),
        }
        let before = fixture.snapshot();
        for result in [
            crate::host_lock::acquire_host_process_lock(&fixture.data, "gateway", "offline")
                .map(|_| ()),
            crate::auth::migrate_principal_root_objects_offline(&fixture.data, &plan, &backup)
                .map(|_| ()),
        ] {
            assert!(result.is_err(), "{state}");
            assert_eq!(fixture.snapshot(), before, "{state}");
        }
    }
}

#[test]
fn retained_signed_receipt_keeps_writer_ownership_when_sources_are_absent() {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    assert!(!crate::sources::trusted_sources_path(&fixture.data).exists());
    assert_eq!(
        installed_writer_binary(&fixture.data).unwrap(),
        Some(fixture.binary.clone())
    );
    let guard =
        crate::install_transaction::InstallationGuard::acquire(fixture.binary.parent().unwrap())
            .unwrap();
    let before = fixture.snapshot();
    assert!(crate::install_transaction::acquire_installed_writer(&fixture.data).is_err());
    assert!(
        crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary))
            .is_err()
    );
    assert_eq!(fixture.snapshot(), before);
    drop(guard);
    let writer = crate::install_transaction::acquire_installed_writer(&fixture.data).unwrap();
    assert!(writer.is_some());
    drop(writer);
    crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary)).unwrap();
    assert_eq!(
        crate::sources::load_trusted_sources(&fixture.data)
            .unwrap()
            .default_source()
            .unwrap()
            .install_path,
        fixture.binary.to_str().unwrap()
    );
}

#[test]
fn retained_receipt_refuses_malformed_or_unsafe_pending_journal_before_source_mutation() {
    for state in ["malformed", "unsafe mode", "symlink", "directory"] {
        let fixture = PrivateFixture::new();
        publish_retained_receipt(&fixture);
        let guard = crate::install_transaction::InstallationGuard::acquire(
            fixture.binary.parent().unwrap(),
        )
        .unwrap();
        drop(guard);
        let journal = fixture
            .binary
            .parent()
            .unwrap()
            .join(".elastos.update-journal.json");
        match state {
            "malformed" => fixture.file(&journal, b"{malformed pending restart", 0o600),
            "unsafe mode" => fixture.file(&journal, b"{malformed pending restart", 0o666),
            "symlink" => symlink(&fixture.binary, &journal).unwrap(),
            "directory" => fs::create_dir(&journal).unwrap(),
            _ => unreachable!(),
        }
        let before = fixture.snapshot();
        assert!(
            crate::install_transaction::acquire_installed_writer(&fixture.data).is_err(),
            "{state}"
        );
        assert!(
            crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary))
                .is_err(),
            "{state}"
        );
        assert_eq!(fixture.snapshot(), before, "{state}");
        assert!(!crate::sources::trusted_sources_path(&fixture.data).exists());
    }
}

#[test]
fn replacement_source_path_cannot_redirect_the_current_installation_lock() {
    for retained_receipt in [false, true] {
        let fixture = PrivateFixture::new();
        crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary))
            .unwrap();
        if retained_receipt {
            publish_retained_receipt(&fixture);
        }
        let next_parent = fixture.data.join("replacement-bin");
        fs::create_dir(&next_parent).unwrap();
        let replacement = source_config(&next_parent.join("runtime"));
        let guard = crate::install_transaction::InstallationGuard::acquire(
            fixture.binary.parent().unwrap(),
        )
        .unwrap();
        let before = fixture.snapshot();
        assert!(crate::sources::save_trusted_sources(&fixture.data, &replacement).is_err());
        assert_eq!(fixture.snapshot(), before);
        assert!(!next_parent.join(".elastos.install.lock").exists());
        drop(guard);
        crate::sources::save_trusted_sources(&fixture.data, &replacement).unwrap();
        assert_eq!(
            crate::sources::load_trusted_sources(&fixture.data)
                .unwrap()
                .default_source()
                .unwrap()
                .install_path,
            next_parent.join("runtime").to_str().unwrap()
        );
    }
}

#[test]
fn fresh_source_persistence_keeps_an_uninstalled_binary_parent_uncreated() {
    let fixture = PrivateFixture::new();
    let data = fixture.data.join("fresh-home");
    let binary = fixture.data.join("uninstalled/bin/runtime");
    assert!(!data.exists());
    let mut config = source_config(&binary);
    crate::sources::save_trusted_sources(&data, &config).unwrap();
    assert!(!binary.parent().unwrap().exists());
    assert!(crate::install_transaction::acquire_installed_writer(&data)
        .unwrap()
        .is_none());
    config.sources[0].channel = "canary".into();
    crate::sources::save_trusted_sources(&data, &config).unwrap();
    assert_eq!(
        crate::sources::load_trusted_sources(&data)
            .unwrap()
            .default_source()
            .unwrap()
            .channel,
        "canary"
    );
    assert!(!binary.parent().unwrap().exists());
}

#[test]
fn installed_controller_admission_binds_signature_channel_version_platform_and_binary() {
    let fixture = PrivateFixture::new();
    let source = test_source();
    let expected = digest(b"signed fixture Runtime");
    let envelope = signed_release(&expected);
    assert_eq!(
        admit_installed_release(&envelope, &source, &fixture.binary).unwrap(),
        expected
    );
    for refusal in [
        "binary",
        "version",
        "channel",
        "publisher",
        "signature",
        "platform",
        "checksum",
    ] {
        let mut source = source.clone();
        let mut value: Value = serde_json::from_slice(&envelope).unwrap();
        let bytes = match refusal {
            "binary" => {
                fs::write(&fixture.binary, b"foreign Runtime").unwrap();
                envelope.clone()
            }
            "version" => {
                source.installed_version = "0.7.2".into();
                envelope.clone()
            }
            "channel" => {
                source.channel = "canary".into();
                envelope.clone()
            }
            "publisher" => {
                source.publisher_dids = vec![crate::crypto::encode_signing_key_did(
                    &ed25519_dalek::SigningKey::from_bytes(&[44; 32]),
                )];
                envelope.clone()
            }
            "signature" => {
                value["signature"] = json!("0".repeat(128));
                serde_json::to_vec(&value).unwrap()
            }
            "platform" => {
                value["payload"]["platforms"] = json!({});
                signed(value["payload"].clone(), "elastos.release.v1")
            }
            "checksum" => {
                value["payload"]["platforms"][crate::update::detect_release_platform()]["binary"]
                    ["sha256"] = json!("bad");
                signed(value["payload"].clone(), "elastos.release.v1")
            }
            _ => unreachable!(),
        };
        let before = fs::read(&fixture.binary).unwrap();
        assert!(
            admit_installed_release(&bytes, &source, &fixture.binary).is_err(),
            "{refusal}"
        );
        assert_eq!(fs::read(&fixture.binary).unwrap(), before);
        fs::write(&fixture.binary, b"signed fixture Runtime").unwrap();
    }
}

#[test]
fn signed_update_choice_requires_the_exact_head_release_publisher_version_and_channel() {
    let (source, choice, head) = choice_fixture();
    verify_update_choice(&head, &choice, &source).unwrap();
    for refusal in [
        "head CID",
        "release CID",
        "version",
        "chosen publisher",
        "trusted publisher",
        "channel",
        "payload tamper",
        "signing domain",
    ] {
        let mut source = source.clone();
        let mut choice = choice.clone();
        let mut value: Value = serde_json::from_slice(&head).unwrap();
        let bytes = match refusal {
            "head CID" => {
                choice.head_cid = raw_cid(b"another head");
                head.clone()
            }
            "release CID" => {
                choice.release_cid = raw_cid(b"another release");
                head.clone()
            }
            "version" => {
                choice.new_version = "0.7.2".into();
                head.clone()
            }
            "chosen publisher" => {
                choice.publisher_did = crate::crypto::encode_signing_key_did(
                    &ed25519_dalek::SigningKey::from_bytes(&[44; 32]),
                );
                head.clone()
            }
            "trusted publisher" => {
                source.publisher_dids = vec![crate::crypto::encode_signing_key_did(
                    &ed25519_dalek::SigningKey::from_bytes(&[44; 32]),
                )];
                head.clone()
            }
            "channel" => {
                value["payload"]["channel"] = json!("canary");
                signed(value["payload"].clone(), "elastos.release.head.v1")
            }
            "payload tamper" => {
                value["payload"]["extra"] = json!("unsigned edit");
                serde_json::to_vec(&value).unwrap()
            }
            "signing domain" => signed(value["payload"].clone(), "elastos.release.v1"),
            _ => unreachable!(),
        };
        if !matches!(refusal, "head CID") {
            choice.head_cid = raw_cid(&bytes);
        }
        assert!(
            verify_update_choice(&bytes, &choice, &source).is_err(),
            "{refusal}"
        );
    }
}

#[test]
fn receipt_round_trip_keeps_non_utf8_launch_args_environment_and_working_directory() {
    let fixture = PrivateFixture::new();
    let cwd = fixture
        .data
        .join(OsString::from_vec(b"working-\xff".to_vec()));
    // APFS rejects this non-UTF8 name; command construction does not access cwd.
    #[cfg(not(target_os = "macos"))]
    fs::create_dir(&cwd).unwrap();
    let encode = |value: &OsStr| BASE64.encode(value.as_bytes());
    let argument = OsString::from_vec(b"argument-\xfe".to_vec());
    let key = OsString::from("PATH");
    let value = OsString::from_vec(b"value-\xfe".to_vec());
    let launch = LaunchPlan {
        args: [
            OsStr::new("home"),
            OsStr::new("--browser"),
            argument.as_os_str(),
        ]
        .map(encode)
        .to_vec(),
        environment: vec![
            (encode(&key), encode(&value)),
            (encode(OsStr::new(HOST_ENV)), encode(OsStr::new("stale"))),
            (
                encode(OsStr::new("ELASTOS_UPDATE_GENERATION")),
                encode(OsStr::new("stale")),
            ),
        ],
        cwd: encode(cwd.as_os_str()),
    };
    let launch_hash = launch.sha256().unwrap();
    let receipt = Receipt {
        schema: "elastos.update-controller/v1".into(),
        data_dir: fixture.data.clone(),
        binary: fixture.binary.clone(),
        controller: fixture.controller(),
        controller_sha256: digest(b"signed fixture Runtime"),
        signed_controller_release: BASE64
            .encode(signed_release(&digest(b"signed fixture Runtime"))),
        trusted_source: test_source(),
        launch,
        launch_sha256: launch_hash.clone(),
    };
    let path = fixture.directory.join(RECEIPT);
    write_private(&path, &receipt).unwrap();
    let receipt: Receipt = read_private_json(&path).unwrap();
    assert_eq!(receipt.launch.sha256().unwrap(), launch_hash);
    assert_eq!(receipt.launch_sha256, launch_hash);
    let generation = "c".repeat(32);
    for restart in [false, true] {
        let command = receipt
            .launch
            .command(&fixture.binary, &generation, restart)
            .unwrap();
        let command = command.as_std();
        assert_eq!(command.get_program(), fixture.binary.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("home"),
                OsStr::new("--browser"),
                argument.as_os_str()
            ]
        );
        assert_eq!(command.get_current_dir(), Some(cwd.as_path()));
        let environment = command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.unwrap().to_os_string()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(environment[&key], value);
        assert_eq!(environment[&OsString::from(HOST_ENV)], "1");
        assert_eq!(
            environment[&OsString::from("ELASTOS_UPDATE_GENERATION")],
            generation.as_str()
        );
        assert_eq!(
            environment[&OsString::from("ELASTOS_UPDATE_RESTART")],
            if restart { "1" } else { "0" }
        );
    }
    let mut bad = receipt.launch.clone();
    bad.args[0] = "invalid base64!".into();
    assert!(bad.command(&fixture.binary, &generation, true).is_err());
    assert_ne!(bad.sha256().unwrap(), receipt.launch_sha256);
}

#[test]
fn launch_capture_and_retained_commands_exclude_unrelated_credentials() {
    let cwd = PathBuf::from("/private/Home fixture");
    let allowed = [
        ("HOME", "/private/Home fixture"),
        ("XDG_DATA_HOME", "/private/Home fixture/data"),
        ("PATH", "/usr/bin:/private/native-support/bin"),
        ("CARGO_HOME", "/private/tools/cargo"),
        ("RUSTUP_HOME", "/private/tools/rustup"),
        ("COINGECKO_DEMO_API_KEY", "fake-wallet-price-key"),
        (
            "ELASTOS_CAPSULE_BIN_DIR",
            "/private/Home fixture/data/elastos/bin",
        ),
        ("ELASTOS_IPFS_KUBO_PATH", "/private/native-support/kubo"),
        (
            "ELASTOS_POLICY_FILE",
            "/private/Home fixture/data/elastos/policy.json",
        ),
        (
            "ELASTOS_HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR",
            "/private/Home fixture/data/elastos",
        ),
        (
            "ELASTOS_HOME_CLI_AUTH_CONTEXT_PROOF_BINDING_ID",
            "private-Home-proof",
        ),
    ];
    let excluded = [
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GITHUB_TOKEN",
        "ELASTOS_AVAILABILITY_AUTHORIZATION",
        "ELASTOS_CARRIER_PEER_ATTESTATION_EXCHANGE_AUTHORIZATION",
        "ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG",
        "ELASTOS_UNKNOWN_API_KEY",
        "ELASTOS_UPDATE_GENERATION",
    ];
    let mut environment = allowed
        .iter()
        .map(|(key, value)| (OsString::from(*key), OsString::from(*value)))
        .collect::<Vec<_>>();
    environment.extend(excluded.iter().map(|key| {
        (
            OsString::from(*key),
            OsString::from("excluded-secret-fixture"),
        )
    }));
    let launch = LaunchPlan::capture_environment(environment, cwd.clone());
    let receipt_bytes = serde_json::to_vec(&launch).unwrap();
    let secret = BASE64.encode(b"excluded-secret-fixture");
    assert!(!receipt_bytes
        .windows(secret.len())
        .any(|window| window == secret.as_bytes()));
    assert_eq!(launch.environment.len(), allowed.len());
    let launch_hash = launch.sha256().unwrap();
    let mut retained: LaunchPlan = serde_json::from_slice(&receipt_bytes).unwrap();
    assert_eq!(retained.sha256().unwrap(), launch_hash);
    // A pre-fix private receipt may still contain these fields. Validate its
    // recorded bytes before the command filters its launch environment.
    retained.environment.extend(excluded.iter().map(|key| {
        (
            BASE64.encode(key.as_bytes()),
            BASE64.encode(b"excluded-secret-fixture"),
        )
    }));
    let retained_hash = retained.sha256().unwrap();
    let command = retained
        .command(Path::new("/private/runtime"), &"a".repeat(32), false)
        .unwrap();
    assert_eq!(retained.sha256().unwrap(), retained_hash);
    let values = command
        .as_std()
        .get_envs()
        .map(|(key, value)| (key.to_os_string(), value.unwrap().to_os_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (key, value) in allowed {
        assert_eq!(values[&OsString::from(key)], value);
    }
    for key in excluded
        .into_iter()
        .filter(|key| *key != "ELASTOS_UPDATE_GENERATION")
    {
        assert!(!values.contains_key(&OsString::from(key)), "{key}");
    }
    assert_eq!(
        values[&OsString::from("ELASTOS_UPDATE_GENERATION")],
        "a".repeat(32).as_str()
    );
}

#[test]
fn approved_home_readiness_urls_keep_the_exact_listener_origin_and_path() {
    for value in [
        "http://localhost:8090/home/",
        "http://127.0.0.1:8090/home/",
        "http://[::1]:8090/home/",
    ] {
        assert_eq!(approved_home_url(value, 8090).unwrap().as_str(), value);
        assert!(approved_home_url(value, 8091).is_err(), "{value}");
    }
    for value in [
        "https://localhost:8090/home/",
        "http://localhost/home/",
        "http://localhost:8091/home/",
        "http://localhost:8090/home",
        "http://localhost:8090/home/?key=fixture",
        "http://localhost:8090/home/#fixture",
        "http://user:fixture@localhost:8090/home/",
        "http://127.0.0.2:8090/home/",
        "http://[::1]:8091/home/",
        "http://[::2]:8090/home/",
        "http://localhost:8090/other/",
        "http://localhost:8090/other/../home/",
        "http://example.test:8090/home/",
        "http://localhost:8090/home/%2e%2e/",
    ] {
        assert!(approved_home_url(value, 8090).is_err(), "{value}");
    }
}

#[tokio::test]
async fn desktop_and_wallet_bindings_reach_captured_and_retained_child_commands() {
    let fixture = PrivateFixture::new();
    let bindings = [
        ("DISPLAY", ":91"),
        ("WAYLAND_DISPLAY", "fixture-wayland"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/private/fixture/bus"),
        ("XAUTHORITY", "/private/fixture/xauthority"),
        ("BROWSER", "/private/fixture/browser --new-window"),
        ("WSL_INTEROP", "/run/fixture/interop.sock"),
        ("WSL_DISTRO_NAME", "FixtureLinux"),
        ("COINGECKO_DEMO_API_KEY", "fake-wallet-price-key"),
    ];
    let mut environment = bindings
        .iter()
        .map(|(key, value)| (OsString::from(*key), OsString::from(*value)))
        .collect::<Vec<_>>();
    environment.extend(
        [
            ("AWS_SECRET_ACCESS_KEY", "unrelated-fixture-secret"),
            ("OPENAI_API_KEY", "unrelated-fixture-secret"),
        ]
        .map(|(key, value)| (OsString::from(key), OsString::from(value))),
    );
    let mut captured = LaunchPlan::capture_environment(environment, fixture.data.clone());
    captured.args = ["-c", r#"
        test -z "${AWS_SECRET_ACCESS_KEY+x}" || exit 90
        test -z "${OPENAI_API_KEY+x}" || exit 91
        printf '%s\n' "$DISPLAY" "$WAYLAND_DISPLAY" "$DBUS_SESSION_BUS_ADDRESS" "$XAUTHORITY" "$BROWSER" "$WSL_INTEROP" "$WSL_DISTRO_NAME" "$COINGECKO_DEMO_API_KEY"
    "#].map(|arg| BASE64.encode(arg.as_bytes())).to_vec();
    let path = fixture.directory.join("desktop-plan.json");
    write_private(&path, &captured).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
    let mut retained: LaunchPlan = read_private_json(&path).unwrap();
    retained
        .environment
        .extend(["AWS_SECRET_ACCESS_KEY", "OPENAI_API_KEY"].map(|key| {
            (
                BASE64.encode(key.as_bytes()),
                BASE64.encode(b"legacy-unrelated-fixture-secret"),
            )
        }));
    let expected = bindings
        .iter()
        .map(|(_, value)| format!("{value}\n"))
        .collect::<String>();
    for plan in [captured, retained] {
        let hash = plan.sha256().unwrap();
        let output = plan
            .command(Path::new("/bin/sh"), &"a".repeat(32), false)
            .unwrap()
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "child rejected its environment: {:?}",
            output.status
        );
        assert_eq!(output.stdout, expected.as_bytes());
        assert_eq!(plan.sha256().unwrap(), hash);
    }
}

#[tokio::test]
async fn readiness_uses_the_reported_ipv6_home_listener_and_checks_its_served_bytes() {
    use axum::{
        routing::{get, post},
        Json, Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fixture = PrivateFixture::new();
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let fetched = Arc::new(AtomicUsize::new(0));
    let observed = fetched.clone();
    let health_observed = fetched.clone();
    let attach_observed = fetched.clone();
    let document = b"owned IPv6 Home document";
    let router = Router::new()
        .route(
            "/api/health",
            get(move || {
                health_observed.fetch_add(1, Ordering::SeqCst);
                async { Json(json!({"version":"0.7.0"})) }
            }),
        )
        .route(
            "/api/auth/attach",
            post(move |Json(input): Json<Value>| {
                attach_observed.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(input["secret"], "fixture-attach-secret");
                    Json(json!({"token":"fixture-attach-token"}))
                }
            }),
        )
        .route(
            "/home/",
            get(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async move { document.as_slice() }
            }),
        );
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let pid = std::process::id();
    let generation = "a".repeat(32);
    let binary_hash = digest(b"fixture owned gateway binary");
    let mut coords = json!({
        "api_url":format!("http://[::1]:{port}"), "home_url":format!("http://[::1]:{port}/home/"),
        "attach_secret":"fixture-attach-secret", "runtime_kind":"gateway",
        "pid":pid, "generation":generation, "binary_sha256":binary_hash,
    });
    let coords_path = crate::runtime_control::gateway_runtime_coord_path(&fixture.data);
    write_private(&coords_path, &coords).unwrap();
    write_private(
        &fixture.data.join("host-process.lock"),
        &json!({"pid":pid, "generation":generation, "role":"gateway"}),
    )
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let installed_hash = digest(document);
    let ready = prove_ready(
        &client,
        &fixture.data,
        port,
        pid,
        &generation,
        "0.7.0",
        &binary_hash,
        &installed_hash,
    )
    .await;
    let wrong_document = prove_ready(
        &client,
        &fixture.data,
        port,
        pid,
        &generation,
        "0.7.0",
        &binary_hash,
        &digest(b"foreign Home document"),
    )
    .await;
    coords["generation"] = json!("b".repeat(32));
    write_private(&coords_path, &coords).unwrap();
    let wrong_generation = prove_ready(
        &client,
        &fixture.data,
        port,
        pid,
        &generation,
        "0.7.0",
        &binary_hash,
        &installed_hash,
    )
    .await;
    coords["generation"] = json!(generation);
    write_private(&coords_path, &coords).unwrap();
    write_private(
        &fixture.data.join("host-process.lock"),
        &json!({"pid":pid, "generation":"b".repeat(32), "role":"gateway"}),
    )
    .unwrap();
    let wrong_host_lock = prove_ready(
        &client,
        &fixture.data,
        port,
        pid,
        &generation,
        "0.7.0",
        &binary_hash,
        &installed_hash,
    )
    .await;
    let _ = shutdown.send(());
    server.await.unwrap();
    assert!(ready.is_ok(), "{ready:?}");
    assert!(wrong_document.is_err());
    assert!(wrong_generation.is_err());
    assert!(wrong_host_lock.is_err());
    assert_eq!(fetched.load(Ordering::SeqCst), 6);
}

#[test]
fn first_start_enospc_at_each_bootstrap_step_preserves_ordinary_host_admission() {
    for step in ["directory", "lease", "writer"] {
        let fixture = PrivateFixture::new();
        let source = fixture.publish_installed_release();
        fs::remove_dir(&fixture.directory).unwrap();
        let result = bootstrap_controller(
            &fixture.data,
            &fixture.binary,
            &source,
            |data| {
                if step == "directory" {
                    Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into())
                } else {
                    controller_directory(data)
                }
            },
            |directory| {
                if step == "lease" {
                    Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into())
                } else {
                    acquire_lease(directory)
                }
            },
            |parent| {
                if step == "writer" {
                    Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into())
                } else {
                    crate::install_transaction::InstallationGuard::acquire(parent)
                }
            },
        )
        .unwrap();
        assert!(result.is_none(), "{step}");
        assert_eq!(
            fs::read(&fixture.binary).unwrap(),
            b"signed fixture Runtime"
        );
        assert!(!fixture.controller().exists());
        assert!(!fixture.directory.join(RECEIPT).exists());
        crate::install_transaction::authorize_host_start_with_generation(
            &fixture.data,
            &fixture.binary,
            None,
            std::process::id(),
        )
        .unwrap();
    }
}

#[test]
fn first_start_enospc_refuses_existing_authority_busy_or_unsafe_bootstrap_state() {
    for state in [
        "runtime",
        RECEIPT,
        REQUEST,
        ACTIVE_REQUEST,
        STATUS,
        "runtime.partial",
        "busy lease",
        "unsafe directory",
        "unsafe lease",
        "nonempty lease",
        "pending journal",
        "invalid signature",
    ] {
        let fixture = PrivateFixture::new();
        let source = fixture.publish_installed_release();
        let mut held = None;
        match state {
            "busy lease" => held = Some(acquire_lease(&fixture.directory).unwrap()),
            "unsafe directory" => {
                fs::set_permissions(&fixture.directory, fs::Permissions::from_mode(0o755)).unwrap()
            }
            "unsafe lease" => {
                symlink(&fixture.binary, fixture.directory.join("controller.lock")).unwrap()
            }
            "nonempty lease" => fixture.file(
                &fixture.directory.join("controller.lock"),
                b"unexpected lease contents",
                0o600,
            ),
            "pending journal" => fixture.file(
                &fixture
                    .binary
                    .parent()
                    .unwrap()
                    .join(".elastos.update-journal.json"),
                b"{}",
                0o600,
            ),
            "invalid signature" => fs::write(
                installation_release_manifest_path(&fixture.data),
                b"invalid signed installed release",
            )
            .unwrap(),
            _ => fixture.file(
                &fixture.directory.join(state),
                b"retained controller authority",
                0o600,
            ),
        }
        let before = fixture.snapshot();
        let result = bootstrap_controller(
            &fixture.data,
            &fixture.binary,
            &source,
            |_| Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into()),
            acquire_lease,
            crate::install_transaction::InstallationGuard::acquire,
        );
        assert!(result.is_err(), "{state}");
        assert_eq!(fixture.snapshot(), before, "{state}");
        drop(held);
    }
}

#[test]
fn first_start_enospc_rechecks_a_journal_created_during_bootstrap() {
    let fixture = PrivateFixture::new();
    let source = fixture.publish_installed_release();
    let journal = fixture
        .binary
        .parent()
        .unwrap()
        .join(".elastos.update-journal.json");
    let result = bootstrap_controller(
        &fixture.data,
        &fixture.binary,
        &source,
        controller_directory,
        acquire_lease,
        |_| {
            fixture.file(&journal, b"{}", 0o600);
            Err(std::io::Error::from_raw_os_error(libc::ENOSPC).into())
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read(journal).unwrap(), b"{}");
    assert!(!fixture.controller().exists());
    assert!(!fixture.directory.join(RECEIPT).exists());
}

#[test]
fn verified_current_controller_reuses_its_inode_without_a_free_space_check() {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    let _lease = acquire_lease(&fixture.directory).unwrap();
    let _writer =
        crate::install_transaction::InstallationGuard::acquire(fixture.binary.parent().unwrap())
            .unwrap();
    let before = fixture.snapshot();
    let identity = fs::metadata(fixture.controller()).unwrap();
    let expected = digest(b"signed fixture Runtime");
    assert!(prepare_controller(
        &fixture.directory,
        &fixture.binary,
        &expected,
        |_, _| panic!("same-hash reuse requested a free-space check")
    )
    .unwrap());
    let after = fs::metadata(fixture.controller()).unwrap();
    assert_eq!((identity.dev(), identity.ino()), (after.dev(), after.ino()));
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn first_controller_low_space_keeps_ordinary_home_available_without_creating_a_controller() {
    for error in [
        anyhow::Error::from(elastos_common::NotEnoughFreeSpace { needed: 1 }),
        anyhow::Error::from(std::io::Error::from_raw_os_error(libc::ENOSPC)),
    ] {
        let fixture = PrivateFixture::new();
        let _lease = acquire_lease(&fixture.directory).unwrap();
        let _writer = crate::install_transaction::InstallationGuard::acquire(
            fixture.binary.parent().unwrap(),
        )
        .unwrap();
        let before = fixture.snapshot();
        assert!(!prepare_controller(
            &fixture.directory,
            &fixture.binary,
            &digest(b"signed fixture Runtime"),
            |_, _| Err(error)
        )
        .unwrap());
        assert_eq!(fixture.snapshot(), before);
        assert!(!fixture.controller().exists());
        assert!(!fixture.directory.join(RECEIPT).exists());
        crate::install_transaction::authorize_host_start_with_generation(
            &fixture.data,
            &fixture.binary,
            None,
            std::process::id(),
        )
        .unwrap();
    }
}

#[test]
fn low_space_cannot_hide_unsafe_controller_state_or_foreign_errors() {
    for state in [
        "symlink",
        "mode",
        "unknown bytes",
        "malformed receipt",
        "unsafe scratch",
        "pending journal",
        "controller FIFO",
        "scratch FIFO",
        "receipt FIFO",
    ] {
        let fixture = PrivateFixture::new();
        let _lease = acquire_lease(&fixture.directory).unwrap();
        let _writer = crate::install_transaction::InstallationGuard::acquire(
            fixture.binary.parent().unwrap(),
        )
        .unwrap();
        match state {
            "symlink" => symlink(&fixture.binary, fixture.controller()).unwrap(),
            "mode" => fixture.file(&fixture.controller(), b"signed fixture Runtime", 0o755),
            "unknown bytes" => {
                fixture.file(&fixture.controller(), b"foreign private controller", 0o700)
            }
            "malformed receipt" => fixture.file(&fixture.directory.join(RECEIPT), b"{}", 0o600),
            "unsafe scratch" => symlink(
                &fixture.binary,
                fixture.controller().with_extension("partial"),
            )
            .unwrap(),
            "pending journal" => fixture.file(
                &fixture
                    .binary
                    .parent()
                    .unwrap()
                    .join(".elastos.update-journal.json"),
                b"{}",
                0o600,
            ),
            "controller FIFO" | "scratch FIFO" | "receipt FIFO" => {
                let (path, mode) = match state {
                    "scratch FIFO" => (fixture.controller().with_extension("partial"), 0o700),
                    "receipt FIFO" => (fixture.directory.join(RECEIPT), 0o600),
                    _ => (fixture.controller(), 0o700),
                };
                let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), mode) }, 0);
            }
            _ => unreachable!(),
        }
        let before = fixture.snapshot();
        assert!(
            prepare_controller(
                &fixture.directory,
                &fixture.binary,
                &digest(b"signed fixture Runtime"),
                |_, _| Err(elastos_common::NotEnoughFreeSpace { needed: 1 }.into())
            )
            .is_err(),
            "{state}"
        );
        assert_eq!(fixture.snapshot(), before, "{state}");
    }
    let fixture = PrivateFixture::new();
    let before = fixture.snapshot();
    assert!(prepare_controller(
        &fixture.directory,
        &fixture.binary,
        &digest(b"signed fixture Runtime"),
        |_, _| Err(std::io::Error::from_raw_os_error(libc::EACCES).into())
    )
    .is_err());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn signed_current_controller_repairs_only_an_admitted_prior_receipt() {
    for tamper in [
        "none",
        "signature",
        "launch",
        "binary path",
        "pending journal",
        "current release",
        "current components",
        "controller bytes",
    ] {
        let fixture = PrivateFixture::new();
        fixture.publish_installed_release();
        publish_retained_receipt(&fixture);
        let _lease = acquire_lease(&fixture.directory).unwrap();
        let _writer = crate::install_transaction::InstallationGuard::acquire(
            fixture.binary.parent().unwrap(),
        )
        .unwrap();
        let path = fixture.directory.join(RECEIPT);
        let mut receipt: Receipt = read_private_json(&path).unwrap();
        let mut source = receipt.trusted_source.clone();
        source.installed_version = "0.7.1".into();
        let current_bytes = b"next signed Runtime";
        let expected = digest(current_bytes);
        let components = fs::read(fixture.data.join("components.json")).unwrap();
        let descriptor = |bytes: &[u8]| json!({"cid": raw_cid(bytes), "sha256": digest(bytes), "size": bytes.len()});
        let mut payload = json!({"schema":"elastos.release/v1", "version":"0.7.1", "channel":"stable", "platforms":{}});
        payload["platforms"][crate::update::detect_release_platform()] = json!({
            "binary": descriptor(current_bytes), "components": descriptor(&components)
        });
        let release = signed(payload, "elastos.release.v1");
        let head = signed(
            json!({
                "schema":"elastos.release.head/v1", "version":"0.7.1", "channel":"stable",
                "latest_release_cid":raw_cid(&release), "release_sha256":digest(&release)
            }),
            "elastos.release.head.v1",
        );
        source.head_cid = raw_cid(&head);
        let mut config = source_config(&fixture.binary);
        config.sources[0] = source.clone();
        fs::write(
            fixture.data.join("sources.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        fs::write(installation_release_head_path(&fixture.data), head).unwrap();
        fs::write(installation_release_manifest_path(&fixture.data), &release).unwrap();
        fs::write(&fixture.binary, current_bytes).unwrap();
        // Simulate a crash after the new controller rename and before the receipt rename.
        fs::write(fixture.controller(), current_bytes).unwrap();
        assert_eq!(
            admit_installed_release(&release, &source, &fixture.binary).unwrap(),
            expected
        );
        match tamper {
            "signature" => {
                receipt.signed_controller_release = BASE64.encode(b"invalid signed prior release")
            }
            "launch" => receipt.launch.cwd = BASE64.encode(b"changed prior launch"),
            "binary path" => receipt.binary = fixture.data.join("foreign-runtime"),
            "pending journal" => fixture.file(
                &fixture
                    .binary
                    .parent()
                    .unwrap()
                    .join(".elastos.update-journal.json"),
                b"{uncertain pending journal",
                0o600,
            ),
            "current release" => fs::write(
                installation_release_manifest_path(&fixture.data),
                b"changed current signed release",
            )
            .unwrap(),
            "current components" => fs::write(
                fixture.data.join("components.json"),
                b"changed current components",
            )
            .unwrap(),
            "controller bytes" => fs::write(fixture.controller(), b"foreign controller").unwrap(),
            _ => {}
        }
        write_private(&path, &receipt).unwrap();
        let before = fixture.snapshot();
        let host = crate::install_transaction::authorize_host_start_with_generation(
            &fixture.data,
            &fixture.binary,
            None,
            std::process::id(),
        );
        if matches!(
            tamper,
            "pending journal" | "current release" | "current components" | "controller bytes"
        ) {
            assert!(host.is_err(), "current repair admission: {tamper}");
            if tamper == "pending journal" {
                assert!(prepare_controller(
                    &fixture.directory,
                    &fixture.binary,
                    &expected,
                    |_, _| panic!("pending journal requested a controller copy"),
                )
                .is_err());
            }
            assert_eq!(fixture.snapshot(), before, "{tamper}");
            continue;
        }
        let result = prepare_controller(&fixture.directory, &fixture.binary, &expected, |_, _| {
            panic!("current signed bytes need no copy")
        });
        if tamper == "none" {
            host.unwrap();
            assert!(result.unwrap());
            assert_eq!(fixture.snapshot(), before);
            receipt.controller_sha256 = expected;
            receipt.signed_controller_release = BASE64.encode(release);
            receipt.trusted_source = source;
            write_private(&path, &receipt).unwrap();
            validate_retained_receipt(&read_private_json::<Receipt>(&path).unwrap()).unwrap();
        } else {
            assert!(host.is_err(), "host admission: {tamper}");
            assert!(result.is_err(), "{tamper}");
            assert_eq!(fixture.snapshot(), before, "{tamper}");
        }
    }
}

#[test]
fn private_controller_records_are_bounded_single_link_and_owner_only() {
    let fixture = PrivateFixture::new();
    let record = fixture.directory.join(REQUEST);
    write_private(&record, &json!({"request":"first"})).unwrap();
    assert_eq!(
        read_private_json::<Value>(&record).unwrap(),
        json!({"request":"first"})
    );
    write_private(&record, &json!({"request":"next"})).unwrap();
    assert_eq!(
        read_private_json::<Value>(&record).unwrap(),
        json!({"request":"next"})
    );
    assert_eq!(fs::metadata(&record).unwrap().mode() & 0o777, 0o600);
    for mode in [0o640, 0o660, 0o4600] {
        fs::set_permissions(&record, fs::Permissions::from_mode(mode)).unwrap();
        let before = fs::read(&record).unwrap();
        assert!(read_private_json::<Value>(&record).is_err());
        assert!(write_private(&record, &json!({"request":"foreign overwrite"})).is_err());
        assert_eq!(fs::read(&record).unwrap(), before);
    }
    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();
    let link = fixture.directory.join("linked-record");
    fs::hard_link(&record, &link).unwrap();
    let before = fs::read(&record).unwrap();
    assert!(read_private_json::<Value>(&record).is_err());
    assert!(write_private(&record, &json!({"request":"foreign overwrite"})).is_err());
    assert_eq!(fs::read(&link).unwrap(), before);
    fs::remove_file(&link).unwrap();
    let symlink_path = fixture.directory.join("symlink-record");
    symlink(&record, &symlink_path).unwrap();
    assert!(read_private_json::<Value>(&symlink_path).is_err());
    assert!(write_private(&symlink_path, &json!({"request":"foreign overwrite"})).is_err());
    assert_eq!(fs::read(&record).unwrap(), before);
    let large = vec![b'x'; MAX_PRIVATE_JSON as usize + 1];
    assert!(write_new_private(&fixture.directory.join("oversized"), &large, 0o600).is_err());
    assert!(!fixture.directory.join("oversized").exists());
    fs::write(&record, large).unwrap();
    assert!(read_private_json::<Value>(&record).is_err());
}

#[test]
fn controller_lease_excludes_a_second_owner_and_keeps_the_same_lock_inode() {
    let fixture = PrivateFixture::new();
    let lease = acquire_lease(&fixture.directory).unwrap();
    let path = fixture.directory.join("controller.lock");
    let identity = fs::metadata(&path).unwrap().ino();
    assert!(acquire_lease(&fixture.directory).is_err());
    drop(lease);
    let _lease = acquire_lease(&fixture.directory).unwrap();
    assert_eq!(fs::metadata(path).unwrap().ino(), identity);
}

#[test]
fn ended_controller_lease_is_free_while_a_command_spawned_under_it_runs() {
    let fixture = PrivateFixture::new();
    let lease = acquire_lease(&fixture.directory).unwrap();
    let _command = SpawnedWhileOpen::new(&fixture.directory.join("controller.lock"));
    drop(lease);
    acquire_lease(&fixture.directory)
        .expect("the next controller must not wait for a command spawned under the last");
}

#[test]
fn consumed_request_completion_preserves_a_different_request_identity() {
    let fixture = PrivateFixture::new();
    let (_, choice, _) = choice_fixture();
    let launch = LaunchPlan {
        args: vec![BASE64.encode(b"home"), BASE64.encode(b"--browser")],
        environment: Vec::new(),
        cwd: BASE64.encode(fixture.data.as_os_str().as_bytes()),
    };
    let receipt = Receipt {
        schema: "elastos.update-controller/v1".into(),
        data_dir: fixture.data.clone(),
        binary: fixture.binary.clone(),
        controller: fixture.controller(),
        controller_sha256: digest(b"signed fixture Runtime"),
        signed_controller_release: BASE64
            .encode(signed_release(&digest(b"signed fixture Runtime"))),
        trusted_source: test_source(),
        launch_sha256: launch.sha256().unwrap(),
        launch,
    };
    let mut controller = Controller {
        receipt,
        directory: fixture.directory.clone(),
        child: None,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
        request: Some(choice.clone()),
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: String::new(),
        host_ready: false,
    };
    let path = fixture.directory.join(ACTIVE_REQUEST);
    let mut different = choice.clone();
    different.id = "d".repeat(32);
    write_private(&path, &different).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(controller.finish_request().is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    write_private(&path, &choice).unwrap();
    controller.finish_request().unwrap();
    assert!(!path.exists());
    controller.finish_request().unwrap();
}

#[tokio::test]
async fn pre_restart_reconciliation_retires_only_consumed_request_after_both_locks_are_available() {
    use crate::install_transaction::ReleaseFile;
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    let old_sources = serde_json::to_vec(&source_config(&fixture.binary)).unwrap();
    let metadata = [
        (
            ReleaseFile::Components,
            fixture.data.join("components.json"),
            b"old components".as_slice(),
        ),
        (
            ReleaseFile::Sources,
            crate::sources::trusted_sources_path(&fixture.data),
            old_sources.as_slice(),
        ),
        (
            ReleaseFile::ReleaseHead,
            elastos_common::localhost::installation_release_head_path(&fixture.data),
            b"old head".as_slice(),
        ),
        (
            ReleaseFile::ReleaseManifest,
            installation_release_manifest_path(&fixture.data),
            b"old release".as_slice(),
        ),
    ];
    for (_, path, bytes) in &metadata {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("release-head.json" | "release.json")
        ) {
            fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fixture.file(path, bytes, 0o600);
    }
    let candidate = [
        (ReleaseFile::RuntimeBinary, b"candidate Runtime".as_slice()),
        (ReleaseFile::Components, b"candidate components".as_slice()),
        (ReleaseFile::Sources, b"candidate sources".as_slice()),
        (ReleaseFile::ReleaseHead, b"candidate head".as_slice()),
        (
            ReleaseFile::ReleaseManifest,
            b"candidate release".as_slice(),
        ),
    ];
    let transaction = InstallTransaction::acquire(&fixture.data, &fixture.binary).unwrap();
    transaction.prepare(&candidate).unwrap();
    drop(transaction);
    fixture.file(
        &fixture.data.join("new-owner-data"),
        b"owner data after staging",
        0o600,
    );
    let (_, choice, _) = choice_fixture();
    let active_path = fixture.directory.join(ACTIVE_REQUEST);
    write_private(&active_path, &choice).unwrap();
    let mut next = choice.clone();
    next.id = "d".repeat(32);
    let queued_path = fixture.directory.join(REQUEST);
    write_private(&queued_path, &next).unwrap();
    let queued_bytes = fs::read(&queued_path).unwrap();
    let mut controller = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: None,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
        request: None,
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: String::new(),
        host_ready: false,
    };
    let host =
        crate::host_lock::acquire_host_process_lock(&fixture.data, "update-recovery", "offline")
            .unwrap();
    let before = fixture.snapshot();
    assert!(controller.reconcile_pending().await.is_err());
    assert_eq!(fixture.snapshot(), before);
    assert!(active_path.is_file());
    drop(host);
    controller.reconcile_pending().await.unwrap();
    assert!(!active_path.exists());
    assert_eq!(fs::read(&queued_path).unwrap(), queued_bytes);
    assert_eq!(
        fs::read(&fixture.binary).unwrap(),
        b"signed fixture Runtime"
    );
    for (_, path, bytes) in &metadata {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
    assert_eq!(
        fs::read(fixture.data.join("new-owner-data")).unwrap(),
        b"owner data after staging"
    );
    assert!(!InstallTransaction::has_pending_recovery(&fixture.binary));
    assert!(controller.child.is_none());
}

#[test]
fn installed_home_digest_selects_the_document_served_for_data_and_wasm_capsules() {
    for capsule_type in ["data", "wasm"] {
        let fixture = PrivateFixture::new();
        let capsule = fixture.data.join("capsules/home");
        fs::create_dir_all(capsule.join("browser")).unwrap();
        fs::create_dir_all(capsule.join("web")).unwrap();
        let entrypoint = if capsule_type == "data" {
            "web/home.html"
        } else {
            "home.wasm"
        };
        let manifest = json!({
            "schema":"elastos.capsule/v1", "name":"home", "version":"0.1.0",
            "description":"Restart document fixture", "author":"fixture",
            "role":"app", "type":capsule_type, "entrypoint":entrypoint
        });
        fixture.file(
            &capsule.join("capsule.json"),
            &serde_json::to_vec(&manifest).unwrap(),
            0o600,
        );
        fixture.file(
            &capsule.join("home.wasm"),
            b"wasm is not the Home document",
            0o600,
        );
        fixture.file(
            &capsule.join("browser/index.html"),
            b"wasm browser document",
            0o600,
        );
        fixture.file(
            &capsule.join("web/home.html"),
            b"declared data document",
            0o600,
        );
        fixture.file(
            &fixture.data.join("components.json"),
            &serde_json::to_vec(&json!({
                "external":{"home":{"install_path":"capsules/home","platforms":{}}},
                "capsules":{}, "profiles":{}
            }))
            .unwrap(),
            0o600,
        );
        let (relative, bytes) = if capsule_type == "data" {
            ("web/home.html", b"declared data document".as_slice())
        } else {
            ("browser/index.html", b"wasm browser document".as_slice())
        };
        let document = fs::canonicalize(capsule.join(relative)).unwrap();
        assert_eq!(
            crate::api::browser_capsules::installed_home_document(&fixture.data),
            Some(document.clone())
        );
        assert_eq!(home_digest(&fixture.data).unwrap(), digest(bytes));
        for foreign in [
            fixture.data.join("foreign-home.html"),
            fixture.data.join("capsules/sibling/index.html"),
        ] {
            fs::create_dir_all(foreign.parent().unwrap()).unwrap();
            fixture.file(&foreign, b"foreign Home document", 0o600);
            fs::remove_file(&document).unwrap();
            symlink(&foreign, &document).unwrap();
            let before = fixture.snapshot();
            assert!(crate::api::browser_capsules::installed_home_document(&fixture.data).is_none());
            assert!(home_digest(&fixture.data).is_err());
            assert_eq!(fixture.snapshot(), before);
        }
    }
}

#[tokio::test]
async fn cancelled_carrier_close_await_keeps_the_same_task_until_cleanup_completes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    let executions = Arc::new(AtomicUsize::new(0));
    let completions = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let task = {
        let executions = executions.clone();
        let completions = completions.clone();
        let entered = entered.clone();
        let release = release.clone();
        tokio::spawn(async move {
            executions.fetch_add(1, Ordering::SeqCst);
            entered.notify_one();
            release.notified().await;
            completions.fetch_add(1, Ordering::SeqCst);
        })
    };
    let task_id = task.id();
    let mut controller = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: None,
        carrier: None,
        carrier_close: Some(task),
        test_readiness: None,
        request: None,
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: String::new(),
        host_ready: false,
    };
    {
        let close = controller.close_carrier();
        tokio::pin!(close);
        tokio::select! {
            biased;
            result = &mut close => panic!("cleanup completed before release: {result:?}"),
            _ = entered.notified() => {},
        }
        // Dropping this await keeps the task in the controller for final cleanup.
    }
    assert_eq!(controller.carrier_close.as_ref().unwrap().id(), task_id);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(completions.load(Ordering::SeqCst), 0);
    release.notify_one();
    controller.close_carrier().await.unwrap();
    assert!(controller.carrier_close.is_none());
    controller.close_carrier().await.unwrap();
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(completions.load(Ordering::SeqCst), 1);
}

#[test]
fn interrupted_controller_copy_retries_only_private_owned_scratch() {
    for complete_scratch in [false, true] {
        let fixture = PrivateFixture::new();
        let controller = fixture.controller();
        let partial = controller.with_extension("partial");
        fixture.file(&controller, b"previous verified controller", 0o700);
        fixture.file(
            &partial,
            if complete_scratch {
                b"signed fixture Runtime"
            } else {
                b"interrupted"
            },
            0o700,
        );
        copy_controller(
            &fixture.binary,
            &controller,
            &digest(b"signed fixture Runtime"),
        )
        .unwrap();
        assert_eq!(fs::read(&controller).unwrap(), b"signed fixture Runtime");
        assert_eq!(fs::metadata(&controller).unwrap().mode() & 0o777, 0o700);
        assert!(!partial.exists());
    }
    for tamper in ["unsafe mode", "hard link", "symlink"] {
        let fixture = PrivateFixture::new();
        let controller = fixture.controller();
        let partial = controller.with_extension("partial");
        let other = fixture.directory.join("foreign-copy");
        fixture.file(&controller, b"previous verified controller", 0o700);
        fixture.file(&other, b"foreign scratch", 0o700);
        match tamper {
            "unsafe mode" => fixture.file(&partial, b"foreign scratch", 0o600),
            "hard link" => fs::hard_link(&other, &partial).unwrap(),
            "symlink" => symlink(&other, &partial).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            copy_controller(
                &fixture.binary,
                &controller,
                &digest(b"signed fixture Runtime")
            )
            .is_err(),
            "{tamper}"
        );
        assert_eq!(
            fs::read(&controller).unwrap(),
            b"previous verified controller"
        );
        assert_eq!(fs::read(&partial).unwrap(), b"foreign scratch");
        assert_eq!(fs::read(&other).unwrap(), b"foreign scratch");
    }
}

#[test]
fn controller_copy_refusals_preserve_the_previous_controller_and_foreign_target() {
    let fixture = PrivateFixture::new();
    let controller = fixture.controller();
    fixture.file(&controller, b"previous verified controller", 0o700);
    assert!(copy_controller(&fixture.binary, &controller, &digest(b"different release")).is_err());
    assert_eq!(
        fs::read(&controller).unwrap(),
        b"previous verified controller"
    );
    assert!(!controller.with_extension("partial").exists());
    fs::remove_file(&controller).unwrap();
    for existing_target in [true, false] {
        let other = fixture.directory.join(if existing_target {
            "existing-target"
        } else {
            "missing-target"
        });
        if existing_target {
            fixture.file(&other, b"foreign controller target", 0o700);
        }
        symlink(&other, &controller).unwrap();
        assert!(copy_controller(
            &fixture.binary,
            &controller,
            &digest(b"signed fixture Runtime")
        )
        .is_err());
        assert_eq!(fs::read_link(&controller).unwrap(), other);
        if existing_target {
            assert_eq!(fs::read(&other).unwrap(), b"foreign controller target");
        }
        assert!(!controller.with_extension("partial").exists());
        fs::remove_file(&controller).unwrap();
    }
}

#[test]
fn log_rotation_checks_private_ownership_before_truncation() {
    for tamper in ["unsafe mode", "hard link", "symlink"] {
        let fixture = PrivateFixture::new();
        let log = fixture.directory.join("runtime.log");
        let other = fixture.directory.join("foreign-log");
        fixture.file(&other, b"foreign retained log", 0o600);
        match tamper {
            "unsafe mode" => fixture.file(&log, b"foreign retained log", 0o640),
            "hard link" => fs::hard_link(&other, &log).unwrap(),
            "symlink" => symlink(&other, &log).unwrap(),
            _ => unreachable!(),
        }
        assert!(open_private_log(&log).is_err(), "{tamper}");
        assert_eq!(fs::read(&log).unwrap(), b"foreign retained log", "{tamper}");
        assert_eq!(
            fs::read(&other).unwrap(),
            b"foreign retained log",
            "{tamper}"
        );
    }
    let fixture = PrivateFixture::new();
    let log = fixture.directory.join("runtime.log");
    fixture.file(&log, b"previous bounded log", 0o600);
    drop(open_private_log(&log).unwrap());
    assert!(fs::read(&log).unwrap().is_empty());
}

#[tokio::test]
async fn readiness_http_response_refuses_status_and_size_before_use() {
    use axum::{body::Body, http::StatusCode, routing::get, Router};
    for (status, size, limit, accepted) in [
        (StatusCode::OK, 64, 64, true),
        (StatusCode::OK, 65, 64, false),
        (StatusCode::FORBIDDEN, 64, 64, false),
        (StatusCode::TEMPORARY_REDIRECT, 0, 64, false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/",
            get(move || async move { (status, Body::from(vec![b'x'; size])) }),
        );
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let response = client
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap();
        let result = response_bytes(response, limit).await;
        let _ = shutdown.send(());
        task.await.unwrap();
        assert_eq!(result.is_ok(), accepted, "status={status} size={size}");
        if accepted {
            assert_eq!(result.unwrap(), vec![b'x'; size]);
        }
    }
}

fn owner_queue_fixture() -> (PrivateFixture, UpdateRequest) {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary)).unwrap();
    let (_, request, _) = choice_fixture();
    publish_owner_queue_status(&fixture, &request, "ready", None);
    assert!(!has_queued_update(&fixture.data).unwrap());
    reserve_owner_update(
        &fixture.data,
        &request,
        "passkey:step-up:fixture",
        &owner_effect_sha(),
        false,
    )
    .unwrap();
    (fixture, request)
}

fn publish_owner_queue_status(
    fixture: &PrivateFixture,
    request: &UpdateRequest,
    phase: &str,
    completed_version: Option<&str>,
) {
    write_private(
        &fixture.directory.join(STATUS),
        &UpdateStatus {
            id: (phase != "ready").then(|| request.id.clone()),
            phase: phase.into(),
            current_version: completed_version.unwrap_or(&request.current_version).into(),
            new_version: (phase != "ready").then(|| request.new_version.clone()),
            message: "isolated owner queue fixture".into(),
            controller_pid: std::process::id(),
            controller_start: process_start(std::process::id()).unwrap(),
            host_pid: Some(std::process::id()),
            generation: "d".repeat(32),
        },
    )
    .unwrap();
}

fn owner_effect_sha() -> String {
    digest(b"isolated exact owner approval intent")
}

#[test]
fn owner_update_queue_records_one_private_effect_and_retries_exactly() {
    let (fixture, request) = owner_queue_fixture();
    queue_owner_update(
        &fixture.data,
        request.clone(),
        "passkey:step-up:fixture",
        &owner_effect_sha(),
    )
    .unwrap();
    assert!(has_queued_update(&fixture.data).unwrap());
    let queued: UpdateRequest = read_private_json(&fixture.directory.join(REQUEST)).unwrap();
    assert_eq!(queued, request);
    let receipt_path = fixture.directory.join("owner-action.json");
    let receipt: OwnerActionReceipt = read_private_json(&receipt_path).unwrap();
    assert!(receipt.queued);
    assert_eq!(receipt.request, request);
    assert_eq!(receipt.effect_id, "passkey:step-up:fixture");
    assert_eq!(receipt.request_sha256, owner_effect_sha());
    assert_eq!(fs::metadata(receipt_path).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(
        fs::metadata(fixture.directory.join(REQUEST))
            .unwrap()
            .mode()
            & 0o7777,
        0o600
    );
    let queued_snapshot = fixture.snapshot();
    for recovered in [false, true] {
        reserve_owner_update(
            &fixture.data,
            &request,
            "passkey:step-up:fixture",
            &owner_effect_sha(),
            recovered,
        )
        .unwrap();
        queue_owner_update(
            &fixture.data,
            request.clone(),
            "passkey:step-up:fixture",
            &owner_effect_sha(),
        )
        .unwrap();
        assert_eq!(fixture.snapshot(), queued_snapshot);
    }
}

#[test]
fn owner_update_recovery_requires_its_retained_effect_receipt() {
    let (fixture, request) = owner_queue_fixture();
    fs::remove_file(fixture.directory.join("owner-action.json")).unwrap();
    assert!(queue_owner_update(
        &fixture.data,
        request,
        "passkey:step-up:missing",
        &owner_effect_sha()
    )
    .is_err());
    for name in [REQUEST, ACTIVE_REQUEST, "owner-action.json"] {
        assert!(
            !fixture.directory.join(name).exists(),
            "recovery wrote {name}"
        );
    }
    assert!(!has_queued_update(&fixture.data).unwrap());
}

#[test]
fn owner_update_replay_binds_effect_hash_and_every_request_field() {
    let (fixture, request) = owner_queue_fixture();
    queue_owner_update(
        &fixture.data,
        request.clone(),
        "passkey:step-up:fixture",
        &owner_effect_sha(),
    )
    .unwrap();
    let before = fixture.snapshot();
    for field in [
        "id",
        "source_name",
        "channel",
        "publisher_did",
        "current_version",
        "new_version",
        "head_cid",
        "release_cid",
    ] {
        let mut changed = serde_json::to_value(&request).unwrap();
        changed[field] = json!("different approved intent");
        let changed = serde_json::from_value(changed).unwrap();
        assert!(
            queue_owner_update(
                &fixture.data,
                changed,
                "passkey:step-up:fixture",
                &owner_effect_sha()
            )
            .is_err(),
            "{field}"
        );
        assert_eq!(fixture.snapshot(), before, "{field}");
    }
    assert!(queue_owner_update(
        &fixture.data,
        request.clone(),
        "passkey:step-up:fixture",
        &digest(b"changed intent")
    )
    .is_err());
    for recovered in [false, true] {
        assert!(reserve_owner_update(
            &fixture.data,
            &request,
            "passkey:step-up:another",
            &owner_effect_sha(),
            recovered
        )
        .is_err());
        assert!(queue_owner_update(
            &fixture.data,
            request.clone(),
            "passkey:step-up:another",
            &owner_effect_sha()
        )
        .is_err());
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn owner_update_recovery_finishes_both_interrupted_queue_receipts() {
    for request_was_written in [false, true] {
        let (fixture, request) = owner_queue_fixture();
        write_private(
            &fixture.directory.join("owner-action.json"),
            &OwnerActionReceipt {
                schema: "elastos.home-update-owner/v1".into(),
                effect_id: "passkey:step-up:fixture".into(),
                request_sha256: owner_effect_sha(),
                request: request.clone(),
                queued: false,
            },
        )
        .unwrap();
        if request_was_written {
            queue_update(&fixture.data, request.clone()).unwrap();
            publish_owner_queue_status(&fixture, &request, "staging", None);
        }
        queue_owner_update(
            &fixture.data,
            request.clone(),
            "passkey:step-up:fixture",
            &owner_effect_sha(),
        )
        .unwrap();
        let queued: UpdateRequest = read_private_json(&fixture.directory.join(REQUEST)).unwrap();
        assert_eq!(queued, request);
        let receipt: OwnerActionReceipt =
            read_private_json(&fixture.directory.join("owner-action.json")).unwrap();
        assert!(receipt.queued);
        assert_eq!(receipt.request, request);
    }
}

#[test]
fn queued_and_active_update_retries_require_the_exact_request() {
    for active in [false, true] {
        let (fixture, request) = owner_queue_fixture();
        queue_owner_update(
            &fixture.data,
            request.clone(),
            "passkey:step-up:fixture",
            &owner_effect_sha(),
        )
        .unwrap();
        if active {
            fs::rename(
                fixture.directory.join(REQUEST),
                fixture.directory.join(ACTIVE_REQUEST),
            )
            .unwrap();
        }
        assert!(has_queued_update(&fixture.data).unwrap());
        publish_owner_queue_status(&fixture, &request, "restarting", None);
        let before = fixture.snapshot();
        queue_update(&fixture.data, request.clone()).unwrap();
        assert_eq!(fixture.snapshot(), before);
        let mut changed = request;
        changed.new_version = "0.7.2".into();
        assert!(queue_update(&fixture.data, changed).is_err());
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn retired_update_retry_requires_terminal_status_bound_to_owner_request() {
    for phase in ["updated", "restored", "failed"] {
        let (fixture, request) = owner_queue_fixture();
        queue_owner_update(
            &fixture.data,
            request.clone(),
            "passkey:step-up:fixture",
            &owner_effect_sha(),
        )
        .unwrap();
        fs::remove_file(fixture.directory.join(REQUEST)).unwrap();
        assert!(!has_queued_update(&fixture.data).unwrap());
        let version = if phase == "updated" { "0.7.1" } else { "0.7.0" };
        publish_owner_queue_status(&fixture, &request, phase, Some(version));
        let before = fixture.snapshot();
        queue_update(&fixture.data, request.clone()).unwrap();
        assert_eq!(fixture.snapshot(), before);
        assert!(!fixture.directory.join(REQUEST).exists());
        let mut changed = request.clone();
        changed.head_cid = raw_cid(b"different retired signed choice");
        assert!(queue_update(&fixture.data, changed).is_err());
        assert_eq!(fixture.snapshot(), before);
        publish_owner_queue_status(&fixture, &request, "staging", None);
        assert!(queue_update(&fixture.data, request.clone()).is_err());
        assert!(!fixture.directory.join(REQUEST).exists());
        publish_owner_queue_status(&fixture, &request, phase, Some(version));
        fs::remove_file(fixture.directory.join("owner-action.json")).unwrap();
        assert!(queue_update(&fixture.data, request).is_err());
        assert!(!fixture.directory.join(REQUEST).exists());
    }
}

#[test]
fn owner_update_refuses_malformed_unsafe_or_substituted_effect_receipts() {
    for refusal in ["schema", "malformed", "unsafe mode", "symlink", "hard link"] {
        let (fixture, request) = owner_queue_fixture();
        let path = fixture.directory.join("owner-action.json");
        // The fixture reserves a valid action first; replace only that fixture-owned file.
        fs::remove_file(&path).unwrap();
        let valid = serde_json::to_vec(&OwnerActionReceipt {
            schema: if refusal == "schema" {
                "foreign receipt"
            } else {
                "elastos.home-update-owner/v1"
            }
            .into(),
            effect_id: "passkey:step-up:fixture".into(),
            request_sha256: owner_effect_sha(),
            request: request.clone(),
            queued: true,
        })
        .unwrap();
        match refusal {
            "schema" => fixture.file(&path, &valid, 0o600),
            "malformed" => fixture.file(&path, b"{incomplete effect", 0o600),
            "unsafe mode" => fixture.file(&path, &valid, 0o640),
            "symlink" | "hard link" => {
                let other = fixture.data.join("foreign-owner-action.json");
                fixture.file(&other, &valid, 0o600);
                if refusal == "symlink" {
                    symlink(other, &path).unwrap();
                } else {
                    fs::hard_link(other, &path).unwrap();
                }
            }
            _ => unreachable!(),
        }
        let before = fixture.snapshot();
        assert!(
            queue_owner_update(
                &fixture.data,
                request,
                "passkey:step-up:fixture",
                &owner_effect_sha()
            )
            .is_err(),
            "{refusal}"
        );
        assert_eq!(fixture.snapshot(), before, "{refusal}");
        assert!(!fixture.directory.join(REQUEST).exists());
    }
}

#[test]
fn second_owner_action_reserves_before_consumption_and_refuses_old_replay() {
    let (fixture, request) = owner_queue_fixture();
    // A completed prior action can retire its queue while keeping its last receipt.
    queue_owner_update(
        &fixture.data,
        request.clone(),
        "passkey:step-up:fixture",
        &owner_effect_sha(),
    )
    .unwrap();
    fs::remove_file(fixture.directory.join(REQUEST)).unwrap();
    reserve_owner_update(
        &fixture.data,
        &request,
        "passkey:step-up:second",
        &owner_effect_sha(),
        false,
    )
    .unwrap();
    let reserved = fixture.snapshot();
    // A crash after reservation, before or after consumption, keeps B retryable.
    for recovered in [false, true] {
        reserve_owner_update(
            &fixture.data,
            &request,
            "passkey:step-up:second",
            &owner_effect_sha(),
            recovered,
        )
        .unwrap();
        assert_eq!(fixture.snapshot(), reserved);
    }
    assert!(reserve_owner_update(
        &fixture.data,
        &request,
        "passkey:step-up:fixture",
        &owner_effect_sha(),
        true
    )
    .is_err());
    assert!(queue_owner_update(
        &fixture.data,
        request.clone(),
        "passkey:step-up:fixture",
        &owner_effect_sha()
    )
    .is_err());
    assert_eq!(fixture.snapshot(), reserved);
    queue_owner_update(
        &fixture.data,
        request,
        "passkey:step-up:second",
        &owner_effect_sha(),
    )
    .unwrap();
    assert!(has_queued_update(&fixture.data).unwrap());
}

#[test]
fn ended_owner_action_frees_its_lock_while_a_command_spawned_during_it_runs() {
    let (fixture, request) = owner_queue_fixture();
    let (directory, action) = owner_action_guard(&fixture.data).unwrap();
    let _command = SpawnedWhileOpen::new(&directory.join("owner-action.lock"));
    drop(action);
    queue_owner_update(
        &fixture.data,
        request,
        "passkey:step-up:fixture",
        &owner_effect_sha(),
    )
    .expect("the next owner action must not wait for a command spawned during the last");
    assert!(has_queued_update(&fixture.data).unwrap());
}

#[test]
fn recovered_consumed_request_reports_terminal_result_after_readiness() {
    let (_, request, _) = choice_fixture();
    assert_eq!(
        initial_ready_result(None, &request.current_version)
            .unwrap()
            .0,
        "ready"
    );
    for (version, phase) in [
        (&request.new_version, "updated"),
        (&request.current_version, "restored"),
    ] {
        assert_eq!(
            initial_ready_result(Some(&request), version).unwrap().0,
            phase
        );
    }
    assert!(initial_ready_result(Some(&request), "unapproved version").is_err());
}

#[tokio::test]
async fn recovery_with_an_owned_ready_child_publishes_its_terminal_result() {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary)).unwrap();
    let (_, request, _) = choice_fixture();
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "exit 0"]);
    let child = child::OwnedChild::spawn(&mut command).unwrap();
    let mut controller = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: Some(child),
        request: Some(request.clone()),
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: "a".repeat(32),
        host_ready: false,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
    };
    // A failed readiness proof retains recovery rather than publishing success.
    assert!(controller.complete_reconciliation().await.is_err());
    assert!(!fixture.directory.join(STATUS).exists());
    controller.host_ready = true;
    controller.complete_reconciliation().await.unwrap();
    let status = status(&fixture.data).unwrap().unwrap();
    assert_eq!(status.phase, "restored");
    assert_eq!(status.id.as_deref(), Some(request.id.as_str()));
    assert_eq!(status.current_version, request.current_version);
    assert!(controller.child.is_some(), "recovered child was replaced");
    controller.stop_child().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn initial_home_readiness_has_a_finite_budget_beyond_the_restart_limit() {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    let capsule = fixture.data.join("capsules/home");
    fs::create_dir_all(capsule.join("browser")).unwrap();
    fixture.file(
        &capsule.join("capsule.json"),
        &serde_json::to_vec(&json!({
            "schema":"elastos.capsule/v1", "name":"home", "version":"0.1.0",
            "description":"Readiness deadline fixture", "author":"fixture",
            "role":"app", "type":"data", "entrypoint":"browser/index.html"
        }))
        .unwrap(),
        0o600,
    );
    fixture.file(
        &capsule.join("browser/index.html"),
        b"private Home document",
        0o600,
    );
    fixture.file(
        &fixture.data.join("components.json"),
        &serde_json::to_vec(&json!({
            "external":{"home":{"install_path":"capsules/home","platforms":{}}},
            "capsules":{}, "profiles":{}
        }))
        .unwrap(),
        0o600,
    );
    assert_eq!(
        home_digest(&fixture.data).unwrap(),
        digest(b"private Home document")
    );
    assert!(!crate::runtime_control::gateway_runtime_coord_path(&fixture.data).exists());
    let mut command = tokio::process::Command::new("/bin/sleep");
    command.arg("600");
    let child = child::OwnedChild::spawn(&mut command).unwrap();
    let pid = child.pid();
    let process_birth = process_start(pid).unwrap();
    let generation = "a".repeat(32);
    let mut controller = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: Some(child),
        request: None,
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: generation.clone(),
        host_ready: false,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
    };

    let binary_hash = digest(b"signed fixture Runtime");
    for restarting in [false, true] {
        let started = tokio::time::Instant::now();
        let message = {
            let proof = controller.wait_ready(
                &generation,
                "0.7.0",
                &binary_hash,
                readiness_budget(restarting),
            );
            tokio::pin!(proof);
            if !restarting {
                tokio::select! {
                    result = &mut proof => panic!("initial readiness ended before 31 seconds: {result:?}"),
                    _ = tokio::time::sleep(Duration::from_secs(31)) => {},
                }
            }
            proof.await.unwrap_err().to_string()
        };
        let expected = Duration::from_secs(if restarting { 30 } else { 120 });
        assert!(started.elapsed() >= expected);
        assert!(started.elapsed() <= expected + Duration::from_millis(50));
        assert!(
            message.contains(&format!("within {} seconds", expected.as_secs())),
            "{message}"
        );
        assert!(!controller.host_ready);
        assert_eq!(controller.child.as_ref().unwrap().pid(), pid);
        assert!(controller
            .child
            .as_ref()
            .unwrap()
            .observed_exit()
            .unwrap()
            .is_none());
        assert!(!fixture.directory.join(STATUS).exists());
    }
    // Restore wall time for the operating system's signal delivery and reap.
    tokio::time::resume();
    controller.stop_child().await.unwrap();
    assert!(controller.child.is_none());
    assert!(child::generation_gone(pid, &process_birth).unwrap());
}

#[tokio::test]
async fn initial_home_loader_failure_names_the_private_log_and_keeps_the_installation() {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    fixture.publish_installed_release();
    let before = fixture.snapshot();
    let mut controller = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: None,
        request: None,
        previous_binary_sha256: String::new(),
        previous_version: String::new(),
        generation: String::new(),
        host_ready: false,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
    };
    // The signed fixture bytes have no native executable format.
    let message = controller.start_initial().await.unwrap_err().to_string();
    assert!(
        message.contains("Home could not start. Check the startup log"),
        "{message}"
    );
    assert!(
        message.contains(fixture.directory.join("runtime.log").to_str().unwrap()),
        "{message}"
    );
    assert!(message.contains("before starting Home again"), "{message}");
    let spawned = controller.child.as_ref().map(|child| {
        let pid = child.pid();
        (
            pid,
            process_start(pid).expect("owned child retains its kernel birth until reap"),
        )
    });
    controller.stop_child().await.unwrap();
    assert!(controller.child.is_none());
    if let Some((pid, birth)) = spawned {
        assert!(child::generation_gone(pid, &birth).unwrap());
    }
    assert!(!controller.host_ready);
    assert!(!fixture.directory.join(STATUS).exists());
    assert!(!InstallTransaction::has_pending_recovery(&fixture.binary));
    let after = fixture.snapshot();
    for (path, entry) in before {
        assert_eq!(after.get(&path), Some(&entry), "{}", path.display());
    }
}

// Observe claims and cleanup while every stop, spawn and readiness proof uses Controller.
struct RealRestartOwner {
    controller: Controller,
    starts: Vec<RestartPhase>,
    stops: usize,
    candidate_process: Option<(u32, Option<String>)>,
    candidate_exit: Option<std::process::ExitStatus>,
    candidate_phase: Option<RestartPhase>,
}

impl crate::update::RestartOwner for RealRestartOwner {
    fn progress(&self, phase: &str, message: &str) -> Result<()> {
        self.controller.publish(phase, message)
    }

    fn plan(&self, support: String, previous: &str, candidate: &str) -> Result<RestartPlan> {
        crate::update::RestartOwner::plan(&self.controller, support, previous, candidate)
    }

    fn stop<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        self.stops += 1;
        // The write occurs after staging and before the candidate's failure.
        if self.stops == 1 {
            fs::write(
                self.controller.receipt.data_dir.join("owner-data"),
                b"new owner data",
            )
            .unwrap();
        }
        crate::update::RestartOwner::stop(&mut self.controller)
    }

    fn start<'a>(
        &'a mut self,
        transaction: &'a InstallTransaction,
        record: RestartRecord,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let candidate = record.phase == RestartPhase::CandidateStartClaimed;
            self.starts.push(record.phase);
            let result =
                crate::update::RestartOwner::start(&mut self.controller, transaction, record).await;
            if candidate {
                let recorded = transaction.restart_record()?;
                self.candidate_phase = Some(recorded.phase);
                self.candidate_exit = self
                    .controller
                    .child
                    .as_ref()
                    .map(|child| child.observed_exit())
                    .transpose()?
                    .flatten();
                self.candidate_process = self.controller.child.as_ref().map(|child| {
                    let pid = child.pid();
                    (pid, process_start(pid).or(recorded.process_start))
                });
            }
            result
        })
    }
}

#[tokio::test]
async fn real_controller_loader_failure_restores_previous_home_once() {
    real_controller_restart_failure("loader").await;
}

#[tokio::test]
async fn real_controller_crash_before_readiness_restores_previous_home_once() {
    real_controller_restart_failure("crash").await;
}

#[tokio::test]
async fn real_controller_hung_start_restores_previous_home_once() {
    real_controller_restart_failure("hung").await;
}

async fn real_controller_restart_failure(failure: &str) {
    let fixture = PrivateFixture::new();
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let previous = format!(
        "#!/bin/sh\nexec {} --exact {} --ignored --nocapture\n",
        quote(std::env::current_exe().unwrap().to_str().unwrap()),
        quote(&format!(
            "{}::restart_home_fixture",
            module_path!().split_once("::").unwrap().1
        )),
    );
    fs::write(&fixture.binary, previous.as_bytes()).unwrap();
    let home = fixture.data.join("capsules/home");
    fs::create_dir_all(home.join("browser")).unwrap();
    fixture.file(
        &home.join("capsule.json"),
        &serde_json::to_vec(&json!({
            "schema":"elastos.capsule/v1", "name":"home", "version":"0.1.0",
            "description":"Restart fixture", "author":"fixture",
            "role":"app", "type":"data", "entrypoint":"browser/index.html"
        }))
        .unwrap(),
        0o600,
    );
    fixture.file(
        &home.join("browser/index.html"),
        b"previous Home document",
        0o600,
    );
    fs::create_dir(fixture.data.join("bin")).unwrap();
    fixture.file(
        &fixture.data.join("bin/fixture-provider"),
        b"old support",
        0o755,
    );
    let components = |support: &[u8]| {
        serde_json::to_vec(&json!({
        "schema":"elastos.components/v1", "capsules":{}, "profiles":{},
        "external":{
            "home":{"install_path":"capsules/home", "platforms":{}},
            "fixture-provider":{"install_path":"bin/fixture-provider", "platforms":{
                (crate::setup::detect_platform()):{
                    "cid":raw_cid(support), "checksum":format!("sha256:{}", digest(support)), "size":support.len()
                }
            }}
        }
    })).unwrap()
    };
    fixture.publish_installed_release_with_components(&components(b"old support"));
    publish_retained_receipt(&fixture);

    let interpreter = fixture.data.join("candidate-interpreter");
    let candidate = if failure == "loader" {
        // A real interpreter admits --version, then disappears before the Home exec.
        // Signed candidate bytes remain unchanged throughout activation and rollback.
        symlink("/bin/sh", &interpreter).unwrap();
        format!(
            "#!{}\n/bin/rm {}\nprintf 'elastos 0.7.1\\n'\n",
            interpreter.display(),
            quote(interpreter.to_str().unwrap())
        )
    } else {
        format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'elastos 0.7.1\\n'; exit 0; fi\nprintf 'candidate\\n' >> candidate-starts\n{}\n",
            if failure == "crash" { "exit 7" } else { "exec /bin/sleep 60" })
    };
    let candidate_components = components(b"new support");
    let descriptor =
        |bytes: &[u8]| json!({"cid":raw_cid(bytes),"sha256":digest(bytes),"size":bytes.len()});
    let release = signed(
        json!({
            "schema":"elastos.release/v1", "version":"0.7.1", "channel":"stable",
            "platforms":{(crate::update::detect_release_platform()):{
                "binary":descriptor(candidate.as_bytes()), "components":descriptor(&candidate_components)
            }}
        }),
        "elastos.release.v1",
    );
    let head = signed(
        json!({
            "schema":"elastos.release.head/v1", "version":"0.7.1", "channel":"stable",
            "latest_release_cid":raw_cid(&release), "release_sha256":digest(&release)
        }),
        "elastos.release.head.v1",
    );
    let (_, mut request, _) = choice_fixture();
    request.head_cid = raw_cid(&head);
    request.release_cid = raw_cid(&release);
    let items = std::collections::BTreeMap::from([
        (raw_cid(&head), head),
        (raw_cid(&release), release),
        (raw_cid(candidate.as_bytes()), candidate.into_bytes()),
        (raw_cid(&candidate_components), candidate_components),
        (raw_cid(b"new support"), b"new support".to_vec()),
    ]);
    let fetch: crate::update::FetchFn = Box::new(move |cid, _| {
        let bytes = items
            .get(&cid)
            .unwrap_or_else(|| panic!("unexpected fetch: {cid}"))
            .clone();
        Box::pin(async move { Ok(bytes) })
    });
    let mut owner = RealRestartOwner {
        controller: Controller {
            receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
            directory: fixture.directory.clone(),
            child: None,
            request: None,
            previous_binary_sha256: digest(previous.as_bytes()),
            previous_version: "0.7.0".into(),
            generation: String::new(),
            host_ready: false,
            carrier: None,
            carrier_close: None,
            test_readiness: Some(Duration::from_secs(3)),
        },
        starts: Vec::new(),
        stops: 0,
        candidate_process: None,
        candidate_exit: None,
        candidate_phase: None,
    };
    owner.controller.start_initial().await.unwrap();
    owner.controller.request = Some(request.clone());
    write_private(&fixture.directory.join(ACTIVE_REQUEST), &request).unwrap();
    let old_paths = [
        fixture.binary.clone(),
        fixture.data.join("components.json"),
        fixture.data.join("sources.json"),
        installation_release_head_path(&fixture.data),
        installation_release_manifest_path(&fixture.data),
        fixture.data.join("bin/fixture-provider"),
        home.join("capsule.json"),
        home.join("browser/index.html"),
    ];
    let old = old_paths.map(|path| {
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    });
    let result =
        crate::update::run_restarting_update(&fixture.data, &fetch, request.head_cid, &mut owner)
            .await;
    owner.controller.publish_apply_result(&result).unwrap();
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains("previous release restored and Home restarted; user data preserved"),
        "{error}"
    );
    match failure {
        "loader" => {
            assert!(error.contains("spawn owned update child"), "{error}");
            assert!(owner.candidate_process.is_none());
            assert_eq!(
                owner.candidate_phase,
                Some(RestartPhase::CandidateStartClaimed)
            );
            assert!(!interpreter.exists());
        }
        "crash" => {
            assert!(error.contains("Home exited"), "{error}");
            assert_eq!(owner.candidate_exit.unwrap().code(), Some(7));
            assert!(matches!(
                owner.candidate_phase,
                Some(RestartPhase::CandidateStartClaimed | RestartPhase::CandidateRunning)
            ));
        }
        "hung" => {
            assert!(
                error.contains("Home did not become ready within 3 seconds"),
                "{error}"
            );
            assert!(owner.candidate_exit.is_none());
            assert_eq!(owner.candidate_phase, Some(RestartPhase::CandidateRunning));
        }
        _ => unreachable!(),
    }
    assert_eq!(
        owner.starts,
        [
            RestartPhase::CandidateStartClaimed,
            RestartPhase::PreviousStartClaimed
        ]
    );
    assert_eq!(owner.stops, 2);
    assert!(owner.controller.host_ready);
    let restored_pid = owner.controller.child.as_ref().unwrap().pid();
    let restored_birth = process_start(restored_pid).unwrap();
    assert!(owner
        .controller
        .child
        .as_ref()
        .unwrap()
        .observed_exit()
        .unwrap()
        .is_none());
    let projected = status(&fixture.data).unwrap().unwrap();
    assert_eq!(projected.phase, "restored");
    assert_eq!(projected.current_version, "0.7.0");
    assert_eq!(projected.host_pid, Some(restored_pid));
    assert_eq!(
        projected.message,
        "The update could not start. Your previous release is ready. Check the update again."
    );
    assert_eq!(
        fs::read(fixture.data.join("previous-starts")).unwrap(),
        b"start\n"
    );
    assert_eq!(
        fs::read(fixture.data.join("previous-ready")).unwrap(),
        b"ready\n"
    );
    if failure != "loader" {
        assert_eq!(
            fs::read(fixture.data.join("candidate-starts")).unwrap(),
            b"candidate\n"
        );
    }
    if let Some((pid, birth)) = &owner.candidate_process {
        if let Some(birth) = birth {
            assert!(child::generation_gone(*pid, birth).unwrap());
        }
        assert_eq!(
            child::observe_exit(*pid).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(child::group_descendants(*pid).unwrap().is_empty());
    }
    for (path, bytes) in old {
        assert_eq!(fs::read(&path).unwrap(), bytes, "{}", path.display());
    }
    assert_eq!(
        fs::read(fixture.data.join("owner-data")).unwrap(),
        b"new owner data"
    );
    for parent in [&fixture.data, &fixture.data.join("installation")] {
        for name in [
            ".elastos.update-journal.json",
            ".elastos.update-journal.tmp",
            ".elastos.update-stage",
            ".elastos.update-rollback",
            ".elastos.update-support",
        ] {
            assert!(
                !parent.join(name).exists(),
                "{}",
                parent.join(name).display()
            );
        }
    }
    assert!(!InstallTransaction::has_pending_recovery(&fixture.binary));
    // Repeated controller reconciliation retains the ready Home and consumes no new start.
    for _ in 0..2 {
        owner.controller.reconcile_pending().await.unwrap();
        owner.controller.complete_reconciliation().await.unwrap();
        assert_eq!(owner.controller.child.as_ref().unwrap().pid(), restored_pid);
        assert_eq!(status(&fixture.data).unwrap().unwrap().phase, "restored");
        assert!(!fixture.directory.join(ACTIVE_REQUEST).exists());
        assert_eq!(
            fs::read(fixture.data.join("previous-starts")).unwrap(),
            b"start\n"
        );
    }
    owner.controller.stop_child().await.unwrap();
    assert!(child::generation_gone(restored_pid, &restored_birth).unwrap());
    assert!(crate::host_lock::active_host_process(&fixture.data)
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "real child Home for controller restart failure tests"]
async fn restart_home_fixture() {
    use axum::{
        routing::{get, post},
        Json, Router,
    };
    let data = std::env::current_dir().unwrap();
    let generation = std::env::var("ELASTOS_UPDATE_GENERATION").unwrap();
    let restarting = std::env::var("ELASTOS_UPDATE_RESTART").unwrap() == "1";
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    child::watch_parent().unwrap();
    crate::install_transaction::authorize_host_start(&data, &data.join("installed-runtime"))
        .unwrap();
    // The signed shell wrapper execs this test harness. Hold the real host lock using
    // the wrapper's admitted identity and this child's PID, rather than current_exe.
    let mut host_lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(data.join("host-process.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(host_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    host_lock.set_len(0).unwrap();
    serde_json::to_writer(
        &mut host_lock,
        &json!({
            "pid":std::process::id(), "generation":generation, "role":"gateway"
        }),
    )
    .unwrap();
    host_lock.sync_all().unwrap();
    if restarting {
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(data.join("previous-starts"))
            .unwrap()
            .write_all(b"start\n")
            .unwrap();
    }
    // A private ephemeral port, away from the operator's Home, reported to the controller.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert_ne!(port, BROWSER_HOME_PORT);
    write_private(&data.join("readiness-port"), &port).unwrap();
    let coords_path = crate::runtime_control::gateway_runtime_coord_path(&data);
    write_private(&coords_path, &json!({
        "api_url":format!("http://127.0.0.1:{port}"), "home_url":format!("http://127.0.0.1:{port}/home/"),
        "attach_secret":"restart-fixture-secret", "runtime_kind":"gateway",
        "pid":std::process::id(), "generation":generation,
        "binary_sha256":digest(&fs::read(data.join("installed-runtime")).unwrap())
    })).unwrap();
    let document = fs::read(data.join("capsules/home/browser/index.html")).unwrap();
    let router = Router::new()
        .route(
            "/api/health",
            get(|| async { Json(json!({"version":"0.7.0"})) }),
        )
        .route(
            "/api/auth/attach",
            post(|Json(input): Json<Value>| async move {
                assert_eq!(input["secret"], "restart-fixture-secret");
                Json(json!({"token":"restart-fixture-token"}))
            }),
        )
        .route(
            "/home/",
            get(move || {
                let document = document.clone();
                let data = data.clone();
                async move {
                    if restarting {
                        OpenOptions::new()
                            .append(true)
                            .create(true)
                            .open(data.join("previous-ready"))
                            .unwrap()
                            .write_all(b"ready\n")
                            .unwrap();
                    }
                    document
                }
            }),
        );
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            term.recv().await;
        })
        .await
        .unwrap();
    // Runtime retires its coordinates before releasing the host process lock.
    fs::remove_file(coords_path).unwrap();
}

#[test]
fn migration_space_refusal_reuses_only_the_exact_current_signed_controller() {
    for case in [
        "current",
        "missing controller",
        "changed controller",
        "unsafe controller",
        "pending journal",
        "partial consumed",
    ] {
        let fixture = PrivateFixture::new();
        let source = fixture.publish_installed_release();
        publish_retained_receipt(&fixture);
        let publisher = elastos_common::localhost::publisher_release_head_path(&fixture.data)
            .parent()
            .unwrap()
            .to_path_buf();
        fs::create_dir_all(publisher.parent().unwrap()).unwrap();
        fs::rename(fixture.data.join("installation"), &publisher).unwrap();
        match case {
            "current" => {}
            "missing controller" => fs::remove_file(fixture.controller()).unwrap(),
            "changed controller" => {
                fs::write(fixture.controller(), b"foreign controller bytes").unwrap()
            }
            "unsafe controller" => {
                fs::set_permissions(fixture.controller(), fs::Permissions::from_mode(0o755))
                    .unwrap()
            }
            "pending journal" => fixture.file(
                &fixture.data.join(".elastos.update-journal.json"),
                b"{}",
                0o600,
            ),
            "partial consumed" => {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(fixture.data.join("installation"))
                    .unwrap();
                fixture.file(
                    &installation_release_head_path(&fixture.data),
                    b"partial consumed state",
                    0o600,
                );
            }
            _ => unreachable!(),
        }
        let _lease = acquire_lease(&fixture.directory).unwrap();
        let _writer = crate::install_transaction::InstallationGuard::acquire(
            fixture.binary.parent().unwrap(),
        )
        .unwrap();
        let before = fixture.snapshot();
        let result = reuse_home_inputs_after_space_refusal(
            &fixture.data,
            &fixture.binary,
            &source,
            &fixture.directory,
            Err(elastos_common::NotEnoughFreeSpace { needed: 1 }.into()),
        );
        assert_eq!(result.is_ok(), case == "current", "{case}");
        assert_eq!(fixture.snapshot(), before, "{case}");
        if case == "current" {
            assert_eq!(
                result.unwrap().binary_sha256,
                digest(b"signed fixture Runtime")
            );
            assert!(!fixture.data.join("installation").exists());
            assert!(!fixture.data.join(".elastos.installation-migrate").exists());
        }
    }
}

#[tokio::test]
async fn stopped_home_with_unreconciled_child_record_reports_failure_and_retains_custody() {
    let fixture = PrivateFixture::new();
    publish_retained_receipt(&fixture);
    crate::sources::save_trusted_sources(&fixture.data, &source_config(&fixture.binary)).unwrap();
    let (_, request, _) = choice_fixture();
    let active = fixture.directory.join(ACTIVE_REQUEST);
    write_private(&active, &request).unwrap();
    let active_bytes = fs::read(&active).unwrap();
    let mut command = tokio::process::Command::new("/bin/sleep");
    command.arg("60");
    let owned = child::OwnedChild::spawn(&mut command).unwrap();
    let pid = owned.pid();
    let birth = process_start(pid).expect("live owned child has a kernel identity");
    let generation = "e".repeat(32);
    let records = fixture
        .data
        .join("gateway-owned-runtimes")
        .join("a".repeat(64));
    fs::create_dir_all(&records).unwrap();
    let record = records.join(format!("{pid}.json"));
    write_private(
        &record,
        &json!({"pid":pid, "process_start":birth,
        "coords_path":fixture.data.join("retained-child-coords.json")}),
    )
    .unwrap();
    let record_bytes = fs::read(&record).unwrap();
    let mut controller = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: Some(owned),
        request: Some(request.clone()),
        previous_binary_sha256: String::new(),
        previous_version: request.current_version.clone(),
        generation: generation.clone(),
        host_ready: true,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
    };
    let stopped = controller.stop_child().await;
    assert!(stopped.is_err());
    assert!(format!("{:#}", stopped.as_ref().unwrap_err()).contains("still need reconciliation"));
    assert!(!controller.host_ready);
    assert_eq!(controller.generation, generation);
    let retained = controller
        .child
        .as_ref()
        .expect("uncertain custody stays with its owner");
    assert_eq!(retained.pid(), pid);
    assert!(retained.observed_exit().unwrap().is_some());
    assert!(child::generation_gone(pid, &birth).unwrap());
    assert_eq!(fs::read(&record).unwrap(), record_bytes);
    assert_eq!(fs::read(&active).unwrap(), active_bytes);
    assert!(controller.publish_ready_result().is_err());
    // Exercise the same status selector used by serve after a failed apply.
    controller.publish_apply_result(&stopped).unwrap();
    let projected = status(&fixture.data).unwrap().unwrap();
    assert_eq!(projected.phase, "failed");
    assert_eq!(projected.id.as_deref(), Some(request.id.as_str()));
    assert!(projected.message.contains("recover"));
    assert!(!projected.message.contains("ready"));
    let mut next_request = request;
    next_request.id = "f".repeat(32);
    assert!(queue_update(&fixture.data, next_request).is_err());
    assert!(!fixture.directory.join(REQUEST).exists());
    assert_eq!(fs::read(&active).unwrap(), active_bytes);
    // Once the test-owned record is reconciled, retry releases the already reaped host.
    fs::remove_file(&record).unwrap();
    controller.stop_child().await.unwrap();
    assert!(controller.child.is_none());
    assert!(!controller.host_ready);
    assert!(controller.generation.is_empty());
    assert!(child::generation_gone(pid, &birth).unwrap());
}

struct StagingRestartOwner {
    controller: Controller,
    host: Option<crate::host_lock::FileLock>,
    stops: usize,
    starts: usize,
    fail_candidate: bool,
    candidate_support: PathBuf,
}

impl crate::update::RestartOwner for StagingRestartOwner {
    fn progress(&self, phase: &str, message: &str) -> Result<()> {
        self.controller.publish(phase, message)
    }
    fn plan(&self, support: String, previous: &str, candidate: &str) -> Result<RestartPlan> {
        crate::update::RestartOwner::plan(&self.controller, support, previous, candidate)
    }
    fn stop<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if self.stops == 0 {
                let data = &self.controller.receipt.data_dir;
                assert_eq!(
                    fs::read(&self.controller.receipt.binary).unwrap(),
                    b"signed fixture Runtime"
                );
                assert_eq!(
                    fs::read(data.join("bin/fixture-provider")).unwrap(),
                    b"old support"
                );
                for (destination, name) in [
                    (self.controller.receipt.binary.clone(), "runtime_binary"),
                    (data.join("components.json"), "components"),
                    (data.join("sources.json"), "sources"),
                    (installation_release_head_path(data), "release_head"),
                    (installation_release_manifest_path(data), "release_manifest"),
                ] {
                    assert!(
                        destination
                            .parent()
                            .unwrap()
                            .join(".elastos.update-stage")
                            .join(name)
                            .is_file(),
                        "{name} is not staged"
                    );
                }
                assert!(fs::read_dir(data.join(".elastos.update-support"))
                    .unwrap()
                    .any(|entry| entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("stage-")));
                assert!(self.controller.child.is_some());
                assert_eq!(status(data)?.unwrap().phase, "restarting");
            }
            self.stops += 1;
            self.host.take();
            self.controller.stop_child().await?;
            Ok(())
        })
    }
    fn start<'a>(
        &'a mut self,
        transaction: &'a InstallTransaction,
        record: RestartRecord,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.starts += 1;
            if self.fail_candidate && record.phase == RestartPhase::CandidateStartClaimed {
                bail!("fixture candidate failed to start");
            }
            let previous = record.phase == RestartPhase::PreviousStartClaimed;
            assert_eq!(
                fs::read(transaction.data_dir().join(if previous {
                    Path::new("bin/fixture-provider")
                } else {
                    &self.candidate_support
                }))?,
                if previous {
                    b"old support".as_slice()
                } else {
                    b"new support".as_slice()
                }
            );
            transaction.verify_support(previous)?;
            let mut command = tokio::process::Command::new("/bin/sh");
            command.args(["-c", "exec sleep 60"]);
            let child = child::OwnedChild::spawn(&mut command)?;
            transaction.record_started(
                &record.generation,
                child.pid(),
                process_start(child.pid()).unwrap(),
            )?;
            self.controller.child = Some(child);
            self.controller.generation = record.generation;
            self.controller.host_ready = true;
            Ok(())
        })
    }
}

#[tokio::test]
async fn staged_support_skips_linux_only_home_component_on_darwin() {
    let fixture = PrivateFixture::new();
    fixture.publish_installed_release();
    publish_retained_receipt(&fixture);
    let mut owner = Controller {
        receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
        directory: fixture.directory.clone(),
        child: None,
        request: None,
        previous_binary_sha256: digest(b"signed fixture Runtime"),
        previous_version: "0.7.0".into(),
        generation: String::new(),
        host_ready: false,
        carrier: None,
        carrier_close: None,
        test_readiness: None,
    };
    let components = serde_json::to_vec(&json!({
        "schema":"elastos.components/v1", "capsules":{},
        "profiles":{"home":{"components":["browser-stream-bridge"]}},
        "external":{"browser-stream-bridge":{
            "install_path":"bin/browser-stream-bridge",
            "platforms":{"linux-arm64":{"cid":raw_cid(b"linux support")}}
        }}
    }))
    .unwrap();
    let fetch: crate::update::FetchFn = Box::new(|_, _| panic!("unavailable support fetched"));
    let (_, paths) = crate::setup::stage_update_support(
        &fixture.data,
        &fs::read(fixture.data.join("components.json")).unwrap(),
        &components,
        "darwin-arm64",
        &fetch,
        &mut owner,
    )
    .await
    .unwrap();
    assert!(paths.is_empty());
}

#[tokio::test]
async fn stage_before_stop_preserves_home_on_fetch_or_verify_failure_and_restarts_once() {
    for outcome in [
        "head fetch",
        "head verify",
        "release fetch",
        "release verify",
        "components fetch",
        "components verify",
        "candidate verify",
        "binary fetch",
        "binary verify",
        "support fetch",
        "support verify",
        "missing checksum",
        "development strategy",
        "optional missing checksum",
        "metadata development strategy",
        "updated",
        "app fetch",
        "app verify",
        "support path",
        "support path fetch",
        "moved",
        "moved restored",
        "bundle",
        "bundle restored",
        "restored",
        "optional restored",
        "optional moved restored",
    ] {
        let fixture = PrivateFixture::new();
        fs::create_dir(fixture.data.join("bin")).unwrap();
        fixture.file(
            &fixture.data.join("bin/fixture-provider"),
            b"old support",
            0o755,
        );
        let manifest = |bytes: &[u8]| {
            serde_json::to_vec(&json!({
            "schema":"elastos.components/v1", "profiles":{}, "capsules":{},
            "external":{"fixture-provider":{"install_path":"bin/fixture-provider", "platforms":{
                (crate::setup::detect_platform()): {"cid":raw_cid(bytes), "checksum":format!("sha256:{}", digest(bytes)), "size":bytes.len()}
            }}}
        })).unwrap()
        };
        fs::create_dir_all(fixture.data.join("capsules/fixture-app")).unwrap();
        fixture.file(
            &fixture.data.join("capsules/fixture-app/capsule.json"),
            b"old app",
            0o644,
        );
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "capsule.json", b"new app".as_slice())
            .unwrap();
        let app = archive.into_inner().unwrap().finish().unwrap();
        let mut old_value: Value = serde_json::from_slice(&manifest(b"old support")).unwrap();
        old_value["capsules"]["fixture-app"] = json!({
            "cid":raw_cid(b"old app"), "sha256":digest(b"old app"), "size":7
        });
        let candidate_support = if outcome.starts_with("moved") {
            PathBuf::from("bin/new-parent/fixture-provider")
        } else if outcome.starts_with("bundle") {
            PathBuf::from("tools/fixture-provider/runner")
        } else {
            PathBuf::from("bin/fixture-provider")
        };
        let mut value: Value = serde_json::from_slice(&manifest(b"new support")).unwrap();
        if outcome.starts_with("optional") {
            let optional = json!({
                "install_path":"libexec/optional/v2/runner",
                "platforms":{(crate::setup::detect_platform()):{
                    "cid":raw_cid(b"optional support"), "checksum":format!("sha256:{}", digest(b"optional support"))
                }}
            });
            if outcome == "optional moved restored" {
                old_value["external"]["optional"] = optional.clone();
                old_value["external"]["optional"]["install_path"] =
                    json!("libexec/optional/v1/runner");
            }
            value["external"]["optional"] = optional;
        }
        value["external"]["fixture-provider"]["install_path"] = json!(candidate_support);
        let support = if outcome.starts_with("bundle") {
            let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
                Vec::new(),
                flate2::Compression::fast(),
            ));
            let mut header = tar::Header::new_gnu();
            header.set_size(11);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(
                    &mut header,
                    "fixture-provider/runner",
                    b"new support".as_slice(),
                )
                .unwrap();
            let bytes = archive.into_inner().unwrap().finish().unwrap();
            value["external"]["fixture-provider"]["install_path"] = json!("tools/fixture-provider");
            value["external"]["fixture-provider"]["platforms"][crate::setup::detect_platform()] = json!({
                "cid":raw_cid(&bytes), "checksum":format!("sha256:{}", digest(&bytes)), "size":bytes.len(),
                "install_path":"tools/fixture-provider", "extract_path":"fixture-provider", "binary_path":"runner"
            });
            bytes
        } else {
            b"new support".to_vec()
        };
        if outcome.starts_with("support path") {
            value["external"]["fixture-provider"]["platforms"][crate::setup::detect_platform()]
                ["release_path"] = json!("fixture-provider");
        }
        value["capsules"]["fixture-app"] = json!({
            "cid":raw_cid(&app), "sha256":digest(&app), "size":app.len()
        });
        if outcome == "missing checksum" {
            value["external"]["fixture-provider"]["platforms"][crate::setup::detect_platform()]
                .as_object_mut()
                .unwrap()
                .remove("checksum");
        } else if outcome == "development strategy" {
            value["external"]["fixture-provider"]["platforms"][crate::setup::detect_platform()]
                ["strategy"] = json!("source-build");
        } else if outcome == "optional missing checksum" {
            value["external"]["optional"] = json!({"platforms":{"*":{"cid":raw_cid(b"optional")}}});
        } else if outcome == "metadata development strategy" {
            value["external"]["fixture-provider"]["capsule_metadata"] = json!({
                "platforms":{"*":{"strategy":"local-copy", "checksum":format!("sha256:{}", digest(b"metadata"))}}
            });
        }
        let old_components = serde_json::to_vec(&old_value).unwrap();
        let components = serde_json::to_vec(&value).unwrap();
        fixture.publish_installed_release_with_components(&old_components);
        publish_retained_receipt(&fixture);
        let binary = if outcome == "candidate verify" {
            b"#!/bin/sh\nprintf 'elastos 0.7.0\\n'\n".as_slice()
        } else {
            b"#!/bin/sh\nprintf 'elastos 0.7.1\\n'\n".as_slice()
        };
        let descriptor =
            |bytes: &[u8]| json!({"cid":raw_cid(bytes),"sha256":digest(bytes),"size":bytes.len()});
        let release = signed(
            json!({
                "schema":"elastos.release/v1","version":"0.7.1","channel":"stable",
                "platforms":{(crate::update::detect_release_platform()):{"binary":descriptor(binary),"components":descriptor(&components)}}
            }),
            "elastos.release.v1",
        );
        let head = signed(
            json!({
                "schema":"elastos.release.head/v1","version":"0.7.1","channel":"stable",
                "latest_release_cid":raw_cid(&release),"release_sha256":digest(&release)
            }),
            "elastos.release.head.v1",
        );
        let (_, mut request, _) = choice_fixture();
        request.head_cid = raw_cid(&head);
        request.release_cid = raw_cid(&release);
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "exec sleep 60"]);
        let child = child::OwnedChild::spawn(&mut command).unwrap();
        let pid = child.pid();
        let birth = process_start(pid).unwrap();
        let mut owner = StagingRestartOwner {
            controller: Controller {
                receipt: read_private_json(&fixture.directory.join(RECEIPT)).unwrap(),
                directory: fixture.directory.clone(),
                child: Some(child),
                request: Some(request.clone()),
                previous_binary_sha256: digest(b"signed fixture Runtime"),
                previous_version: "0.7.0".into(),
                generation: "a".repeat(32),
                host_ready: true,
                carrier: None,
                carrier_close: None,
                test_readiness: None,
            },
            host: Some(
                crate::host_lock::acquire_host_process_lock(&fixture.data, "home", "fixture")
                    .unwrap(),
            ),
            stops: 0,
            starts: 0,
            fail_candidate: outcome.ends_with("restored"),
            candidate_support: candidate_support.clone(),
        };
        owner
            .controller
            .publish("staging", "Checking the signed update.")
            .unwrap();
        let data = fixture.data.clone();
        let original_binary = fixture.binary.clone();
        let binary_cid = raw_cid(binary);
        let support_cid = if outcome.starts_with("support path") {
            "release-path:fixture-provider".into()
        } else {
            raw_cid(&support)
        };
        let head_cid = request.head_cid.clone();
        let release_cid = request.release_cid.clone();
        let components_cid = raw_cid(&components);
        let app_cid = raw_cid(&app);
        let original_sources = fs::read(fixture.data.join("sources.json")).unwrap();
        let items = std::collections::BTreeMap::from([
            (request.head_cid.clone(), head),
            (request.release_cid.clone(), release),
            (binary_cid.clone(), binary.to_vec()),
            (raw_cid(&components), components),
            (support_cid.clone(), support),
            (app_cid.clone(), app),
        ]);
        let fetch: crate::update::FetchFn = Box::new(move |cid, _| {
            if [
                "missing checksum",
                "development strategy",
                "optional missing checksum",
                "metadata development strategy",
            ]
            .contains(&outcome)
            {
                assert!(
                    cid != support_cid && cid != app_cid,
                    "support fetched before manifest admission: {outcome}"
                );
            }
            let bytes = items.get(&cid).unwrap().clone();
            let data = data.clone();
            let original_binary = original_binary.clone();
            let birth = birth.clone();
            let binary_fetch = cid == binary_cid;
            let support_fetch = cid == support_cid;
            let app_fetch = cid == app_cid;
            let failure_target = match outcome.split_whitespace().next().unwrap() {
                "head" => cid == head_cid,
                "release" => cid == release_cid,
                "components" => cid == components_cid,
                "binary" => binary_fetch,
                "support" => support_fetch,
                "app" => app_fetch,
                _ => false,
            };
            Box::pin(async move {
                assert!(
                    !child::generation_gone(pid, &birth).unwrap(),
                    "Home stopped during fetch: {outcome}"
                );
                assert_eq!(
                    fs::read(original_binary).unwrap(),
                    b"signed fixture Runtime"
                );
                assert_eq!(
                    fs::read(data.join("bin/fixture-provider")).unwrap(),
                    b"old support"
                );
                assert_eq!(
                    fs::read(data.join("capsules/fixture-app/capsule.json")).unwrap(),
                    b"old app"
                );
                if binary_fetch || support_fetch || app_fetch {
                    let status = status(&data)?.unwrap();
                    assert_eq!(status.phase, "downloading");
                    assert_eq!(status.message, format!(
                        "Downloading the update ({} B). Home restarts when the update is ready.", bytes.len()
                    ));
                }
                if outcome.ends_with("fetch") && failure_target {
                    return Err(crate::update::UpdateSourceUnavailable.into());
                }
                if outcome.ends_with("verify") && failure_target {
                    return Ok(b"tampered fixture".to_vec());
                }
                Ok(bytes)
            })
        });
        let result = crate::update::run_restarting_update(
            &fixture.data,
            &fetch,
            request.head_cid,
            &mut owner,
        )
        .await;
        owner.controller.publish_apply_result(&result).unwrap();
        let status = status(&fixture.data).unwrap().unwrap();
        if ["updated", "support path", "moved", "bundle"].contains(&outcome) {
            result.unwrap();
            assert_eq!(
                fs::read(fixture.data.join("capsules/fixture-app/capsule.json")).unwrap(),
                b"new app"
            );
            assert_eq!((owner.stops, owner.starts), (1, 1));
            assert_eq!(status.phase, "updated");
            assert_eq!(fs::read(&fixture.binary).unwrap(), binary);
            assert_eq!(
                fs::read(fixture.data.join(&candidate_support)).unwrap(),
                b"new support"
            );
        } else if outcome.ends_with("restored") {
            assert!(result.is_err());
            assert_eq!((owner.stops, owner.starts), (2, 2));
            assert_eq!(status.phase, "restored");
            assert!(owner.controller.child.is_some(), "previous Home is running");
            if outcome.starts_with("optional") {
                assert!(!fixture.data.join("libexec").exists());
            }
            assert_eq!(
                fs::read(&fixture.binary).unwrap(),
                b"signed fixture Runtime"
            );
            assert_eq!(
                fs::read(fixture.data.join("components.json")).unwrap(),
                old_components
            );
            assert_eq!(
                fs::read(fixture.data.join("bin/fixture-provider")).unwrap(),
                b"old support"
            );
            if outcome.starts_with("moved") {
                assert!(!fixture.data.join("bin/new-parent").exists());
            }
            if outcome.starts_with("bundle") {
                assert!(!fixture.data.join("tools").exists());
            }
        } else {
            assert!(result.is_err(), "{outcome}");
            assert_eq!((owner.stops, owner.starts), (0, 0), "{outcome}");
            assert_eq!(owner.controller.child.as_ref().unwrap().pid(), pid);
            assert_eq!(status.phase, "failed");
            assert!(status.message.contains("unchanged"));
            assert_eq!(
                fs::read(fixture.data.join("sources.json")).unwrap(),
                original_sources
            );
            if outcome.ends_with("fetch") {
                assert!(status
                    .message
                    .contains("Connect to the internet and select Update again."));
                assert!(!fixture.directory.join(REFUSED).exists(), "{outcome}");
            }
            if [
                "head verify",
                "release verify",
                "components verify",
                "binary verify",
            ]
            .contains(&outcome)
            {
                // Fetched bytes that differ from the signed digest may be transport damage.
                assert_eq!(
                    status.message,
                    ApplyFailure::DownloadMismatch.message(),
                    "{outcome}"
                );
                assert!(!fixture.directory.join(REFUSED).exists(), "{outcome}");
            }
            if outcome == "head verify" {
                // A publisher-statement refusal binds the exact release; others stay offered.
                let first = owner.controller.request.clone().unwrap();
                let signature = || {
                    crate::update::refused::<()>(Err(anyhow::anyhow!(
                        "Signed envelope signature verification failed"
                    )))
                };
                owner.controller.publish("staging", "Checking.").unwrap();
                owner.controller.publish_apply_result(&signature()).unwrap();
                let refused = super::status(&fixture.data).unwrap().unwrap();
                assert_eq!(refused.message, REFUSED_MESSAGE);
                let mut second = first.clone();
                second.head_cid = "second-head".into();
                second.release_cid = "second-release".into();
                owner.controller.request = Some(second.clone());
                owner.controller.publish("staging", "Checking.").unwrap();
                owner.controller.publish_apply_result(&signature()).unwrap();
                for request in [&first, &second] {
                    assert!(release_refused(&fixture.data, &request.head_cid, "other").unwrap());
                    assert!(release_refused(&fixture.data, "other", &request.release_cid).unwrap());
                }
                assert!(!release_refused(&fixture.data, "next-head", "next-release").unwrap());
                // Installing a release clears only that release's refusal.
                owner.controller.publish_apply_result(&Ok(())).unwrap();
                assert!(!release_refused(&fixture.data, &second.head_cid, "other").unwrap());
                assert!(release_refused(&fixture.data, &first.head_cid, "other").unwrap());
                owner.controller.request = Some(first);
            }
            assert_eq!(
                fs::read(&fixture.binary).unwrap(),
                b"signed fixture Runtime"
            );
            assert_eq!(
                fs::read(fixture.data.join("components.json")).unwrap(),
                old_components
            );
        }
        assert!(
            !InstallTransaction::has_pending_recovery(&fixture.binary),
            "{outcome}"
        );
        assert!(!fixture.data.join(".elastos.update-support").exists());
        if !["updated", "support path", "moved", "bundle"].contains(&outcome) {
            assert_eq!(
                fs::read(fixture.data.join("capsules/fixture-app/capsule.json")).unwrap(),
                b"old app"
            );
        }
        owner.host.take();
        owner.controller.stop_child().await.unwrap();
    }
}

#[test]
fn apply_failures_are_classified_only_from_typed_evidence() {
    for (error, expected) in [
        (
            anyhow::Error::from(crate::update::UpdateSourceUnavailable).context("private endpoint"),
            ApplyFailure::SourceUnavailable,
        ),
        (
            anyhow::Error::from(elastos_common::NotEnoughFreeSpace { needed: 1 }),
            ApplyFailure::NotEnoughSpace,
        ),
        (
            anyhow::Error::from(std::io::Error::from_raw_os_error(libc::ENOSPC)),
            ApplyFailure::NotEnoughSpace,
        ),
        (
            crate::update::mismatched::<()>(Err(anyhow::anyhow!("Binary SHA-256 mismatch!")))
                .unwrap_err()
                .context("staging"),
            ApplyFailure::DownloadMismatch,
        ),
        (
            crate::update::refused::<()>(Err(anyhow::anyhow!(
                "Signed envelope signature verification failed"
            )))
            .unwrap_err()
            .context("staging"),
            ApplyFailure::Refused,
        ),
        (
            anyhow::anyhow!("Binary SHA-256 mismatch!"),
            ApplyFailure::Unknown,
        ),
        (anyhow::anyhow!("unexpected"), ApplyFailure::Unknown),
    ] {
        assert_eq!(classify_apply_failure(&error), expected, "{error:#}");
    }
    let refused =
        crate::update::refused::<()>(Err(anyhow::anyhow!("Binary SHA-256 mismatch!"))).unwrap_err();
    assert_eq!(refused.to_string(), "Binary SHA-256 mismatch!");
    for failure in [
        ApplyFailure::SourceUnavailable,
        ApplyFailure::NotEnoughSpace,
        ApplyFailure::DownloadMismatch,
        ApplyFailure::Unknown,
    ] {
        assert!(failure
            .message()
            .to_lowercase()
            .contains("select update again."));
        assert!(!failure.message().contains("refused"));
    }
    assert!(ApplyFailure::NotEnoughSpace
        .message()
        .contains("free disk space"));
    assert!(!REFUSED_MESSAGE
        .to_lowercase()
        .contains("select update again"));
}

#[test]
fn refused_releases_live_outside_status_and_keep_the_latest_eight() {
    let fixture = PrivateFixture::new();
    // The status format older controllers wrote and read stays exactly the same.
    let older: UpdateStatus = serde_json::from_value(serde_json::json!({
        "id": null, "phase": "failed", "current_version": "0.7.0", "new_version": null,
        "message": "older controller", "controller_pid": 1, "controller_start": "s",
        "host_pid": null, "generation": "",
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&older)
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        9
    );
    let request = |n: usize| UpdateRequest {
        id: "a".repeat(32),
        source_name: "s".into(),
        channel: "stable".into(),
        publisher_did: "did".into(),
        current_version: "0.7.0".into(),
        new_version: "0.7.1".into(),
        head_cid: format!("head-{n}"),
        release_cid: format!("release-{n}"),
    };
    for n in 0..10 {
        record_refused_release(&fixture.directory, &request(n)).unwrap();
    }
    record_refused_release(&fixture.directory, &request(9)).unwrap();
    let kept = read_refused_releases(&fixture.directory).unwrap();
    assert_eq!(kept.len(), MAX_REFUSED);
    assert_eq!(kept.first().unwrap().head_cid, "head-2");
    assert_eq!(kept.last().unwrap().head_cid, "head-9");
    assert!(!release_refused(&fixture.data, "head-1", "release-1").unwrap());
    assert!(release_refused(&fixture.data, "head-2", "x").unwrap());
    forget_refused_release(&fixture.directory, &request(5)).unwrap();
    assert!(!release_refused(&fixture.data, "head-5", "release-5").unwrap());
    assert!(release_refused(&fixture.data, "head-6", "release-6").unwrap());
}
