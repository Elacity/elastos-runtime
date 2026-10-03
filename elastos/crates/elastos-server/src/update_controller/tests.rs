use super::*;
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
    let expected = digest(b"signed fixture Runtime");
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
    fs::create_dir(&cwd).unwrap();
    let encode = |value: &OsStr| BASE64.encode(value.as_bytes());
    let argument = OsString::from_vec(b"argument-\xfe".to_vec());
    let key = OsString::from_vec(b"FIXTURE_\xff".to_vec());
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
            elastos_common::localhost::publisher_release_head_path(&fixture.data),
            b"old head".as_slice(),
        ),
        (
            ReleaseFile::ReleaseManifest,
            publisher_release_manifest_path(&fixture.data),
            b"old release".as_slice(),
        ),
    ];
    for (_, path, bytes) in &metadata {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
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
    let mut command = tokio::process::Command::new("/bin/true");
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
