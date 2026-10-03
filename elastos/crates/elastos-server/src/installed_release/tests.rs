use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

use serde_json::{json, Value};

struct Fixture {
    _root: tempfile::TempDir,
    data: std::path::PathBuf,
    binary: std::path::PathBuf,
    source: TrustedSource,
    head: Vec<u8>,
    release: Vec<u8>,
}

fn key(value: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[value; 32])
}
fn signed(payload: Value, domain: &str, value: u8) -> Vec<u8> {
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &key(value),
        domain,
        &serde_json::to_vec(&payload).unwrap(),
    );
    serde_json::to_vec(
        &json!({"payload": payload, "signature": signature, "signer_did": signer_did}),
    )
    .unwrap()
}
fn raw_cid(bytes: &[u8]) -> String {
    cid::Cid::new_v1(
        0x55,
        cid::multihash::Multihash::<64>::wrap(0x12, &Sha256::digest(bytes)).unwrap(),
    )
    .to_string()
}
fn descriptor(bytes: &[u8]) -> Value {
    json!({"cid": raw_cid(bytes), "sha256": hex::encode(Sha256::digest(bytes)), "size": bytes.len()})
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let data = fs::canonicalize(root.path()).unwrap();
        let binary = data.join("runtime");
        fs::write(&binary, b"installed signed runtime").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let components =
            br#"{"schema":"elastos.components/v1","external":{},"capsules":{},"profiles":{}}"#;
        fs::write(data.join("components.json"), components).unwrap();
        let release = signed(
            json!({"schema":"elastos.release/v1", "version":"0.7.0", "channel":"stable",
            "platforms": {(crate::update::detect_release_platform()): {"binary":descriptor(b"installed signed runtime"), "components":descriptor(components)}}}),
            "elastos.release.v1",
            71,
        );
        let head = signed(
            json!({"schema":"elastos.release.head/v1", "version":"0.7.0", "channel":"stable",
            "latest_release_cid":raw_cid(&release), "release_sha256":hex::encode(Sha256::digest(&release))}),
            "elastos.release.head.v1",
            71,
        );
        let source: TrustedSource = serde_json::from_value(json!({"name":"fixture", "publisher_dids":[crate::crypto::encode_signing_key_did(&key(71))],
            "channel":"stable", "installed_version":"0.7.0", "install_path":binary, "head_cid":raw_cid(&head)})).unwrap();
        let fixture = Self {
            _root: root,
            data,
            binary,
            source,
            head,
            release,
        };
        fixture.publish();
        fixture
    }

    fn publish(&self) {
        fs::create_dir_all(publisher_release_head_path(&self.data).parent().unwrap()).unwrap();
        fs::write(publisher_release_head_path(&self.data), &self.head).unwrap();
        fs::write(publisher_release_manifest_path(&self.data), &self.release).unwrap();
        self.save_source();
    }

    fn bind_components(&mut self) {
        let components = fs::read(self.data.join("components.json")).unwrap();
        let mut release: Value = serde_json::from_slice(&self.release).unwrap();
        release["payload"]["platforms"][crate::update::detect_release_platform()]["components"] =
            descriptor(&components);
        self.release = signed(release["payload"].take(), "elastos.release.v1", 71);
        let mut head: Value = serde_json::from_slice(&self.head).unwrap();
        head["payload"]["latest_release_cid"] = json!(raw_cid(&self.release));
        head["payload"]["release_sha256"] = json!(hex::encode(Sha256::digest(&self.release)));
        self.head = signed(head["payload"].take(), "elastos.release.head.v1", 71);
        self.source.head_cid = raw_cid(&self.head);
        self.publish();
    }

    fn save_source(&self) {
        fs::write(
            self.data.join("sources.json"),
            serde_json::to_vec(&crate::sources::TrustedSourcesConfig {
                schema: "elastos.trusted-sources/v1".into(),
                default_source: self.source.name.clone(),
                sources: vec![self.source.clone()],
            })
            .unwrap(),
        )
        .unwrap();
    }

    fn load(&self) -> Result<InstalledRelease> {
        let guard = InstallationGuard::acquire(self.binary.parent().unwrap())?;
        load_or_migrate(&self.data, &self.binary, &self.source, &guard)
    }

    fn stage(&self) -> std::path::PathBuf {
        let stage = self.data.join(MIGRATION);
        fs::DirBuilder::new().mode(0o700).create(&stage).unwrap();
        stage
    }
}

#[test]
fn migration_preserves_publisher_and_consumed_pair_remains_authoritative() {
    let fixture = Fixture::new();
    let result = fixture.load().unwrap();
    assert_eq!(result.head, fixture.head);
    assert_eq!(result.release, fixture.release);
    assert_eq!(
        fs::read(publisher_release_head_path(&fixture.data)).unwrap(),
        fixture.head
    );
    assert_eq!(
        fs::read(publisher_release_manifest_path(&fixture.data)).unwrap(),
        fixture.release
    );
    assert_eq!(
        fs::metadata(fixture.data.join("installation"))
            .unwrap()
            .mode()
            & 0o7777,
        0o700
    );
    assert_eq!(
        fs::metadata(installation_release_head_path(&fixture.data))
            .unwrap()
            .mode()
            & 0o7777,
        0o600
    );
    assert!(!fixture.data.join(MIGRATION).exists());
    fs::write(
        publisher_release_head_path(&fixture.data),
        b"later publisher output",
    )
    .unwrap();
    fs::write(
        publisher_release_manifest_path(&fixture.data),
        b"later publisher output",
    )
    .unwrap();
    assert_eq!(fixture.load().unwrap().release, fixture.release);
}

#[test]
fn older_empty_head_pin_and_still_pinned_secondary_signer_are_admitted() {
    let mut fixture = Fixture::new();
    fixture.source.head_cid.clear();
    fixture
        .source
        .publisher_dids
        .insert(0, crate::crypto::encode_signing_key_did(&key(72)));
    fixture.save_source();
    assert_eq!(
        read_without_migration(&fixture.data, &fixture.binary, &fixture.source)
            .unwrap()
            .release,
        fixture.release
    );
    assert!(!fixture.data.join("installation").exists());
    assert_eq!(fixture.load().unwrap().head, fixture.head);
}

#[test]
fn complete_pair_admission_refuses_every_changed_binding_before_migration() {
    for case in [
        "head signer",
        "release signer",
        "channel",
        "version",
        "head cid",
        "release cid",
        "release digest",
        "platform",
        "binary",
        "components",
        "size",
    ] {
        let mut fixture = Fixture::new();
        let mut head: Value = serde_json::from_slice(&fixture.head).unwrap();
        let mut release: Value = serde_json::from_slice(&fixture.release).unwrap();
        match case {
            "head signer" => {
                fixture.head = signed(head["payload"].take(), "elastos.release.head.v1", 72)
            }
            "release signer" => {
                fixture.release = signed(release["payload"].take(), "elastos.release.v1", 72)
            }
            "channel" => fixture.source.channel = "canary".into(),
            "version" => fixture.source.installed_version = "0.7.1".into(),
            "head cid" => fixture.source.head_cid = raw_cid(b"other head"),
            "release cid" => {
                head["payload"]["latest_release_cid"] = json!(raw_cid(b"other release"));
                fixture.head = signed(head["payload"].take(), "elastos.release.head.v1", 71);
            }
            "release digest" => {
                head["payload"]["release_sha256"] = json!("0".repeat(64));
                fixture.head = signed(head["payload"].take(), "elastos.release.head.v1", 71);
            }
            "platform" => {
                release["payload"]["platforms"] = json!({});
                fixture.release = signed(release["payload"].take(), "elastos.release.v1", 71);
            }
            "binary" => fs::write(&fixture.binary, b"changed runtime").unwrap(),
            "components" => {
                fs::write(fixture.data.join("components.json"), b"changed components").unwrap()
            }
            "size" => {
                release["payload"]["platforms"][crate::update::detect_release_platform()]
                    ["binary"]["size"] = json!(1);
                fixture.release = signed(release["payload"].take(), "elastos.release.v1", 71);
            }
            _ => unreachable!(),
        }
        if matches!(case, "release signer" | "platform" | "size") {
            head["payload"]["latest_release_cid"] = json!(raw_cid(&fixture.release));
            head["payload"]["release_sha256"] =
                json!(hex::encode(Sha256::digest(&fixture.release)));
            fixture.head = signed(head["payload"].take(), "elastos.release.head.v1", 71);
        }
        if case != "head cid" {
            fixture.source.head_cid = raw_cid(&fixture.head);
        }
        fixture.publish();
        assert!(fixture.load().is_err(), "{case}");
        assert!(!fixture.data.join("installation").exists(), "{case}");
        assert!(!fixture.data.join(MIGRATION).exists(), "{case}");
    }
}

#[test]
fn partial_or_unsafe_consumed_state_has_no_publisher_fallback() {
    for case in [
        "missing release",
        "invalid release",
        "symlink",
        "hardlink",
        "mode",
        "directory mode",
    ] {
        let fixture = Fixture::new();
        fixture.load().unwrap();
        let release = installation_release_manifest_path(&fixture.data);
        match case {
            "missing release" => fs::remove_file(&release).unwrap(),
            "invalid release" => fs::write(&release, b"invalid").unwrap(),
            "symlink" => {
                fs::remove_file(&release).unwrap();
                symlink(publisher_release_manifest_path(&fixture.data), &release).unwrap();
            }
            "hardlink" => {
                fs::remove_file(&release).unwrap();
                fs::hard_link(publisher_release_manifest_path(&fixture.data), &release).unwrap();
            }
            "mode" => fs::set_permissions(&release, fs::Permissions::from_mode(0o644)).unwrap(),
            "directory mode" => fs::set_permissions(
                fixture.data.join("installation"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        assert!(fixture.load().is_err(), "{case}");
        assert_eq!(
            fs::read(publisher_release_manifest_path(&fixture.data)).unwrap(),
            fixture.release,
            "{case}"
        );
    }
}

#[test]
fn private_interrupted_migration_resumes_and_foreign_scratch_is_preserved() {
    for case in [
        "complete",
        "partial",
        "empty destination",
        "foreign bytes",
        "foreign name",
        "symlink",
    ] {
        let fixture = Fixture::new();
        if case == "empty destination" {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(fixture.data.join("installation"))
                .unwrap();
        } else {
            let stage = fixture.stage();
            match case {
                "complete" => {
                    write_private(&stage.join(HEAD), &fixture.head).unwrap();
                    write_private(&stage.join(RELEASE), &fixture.release).unwrap();
                }
                "partial" => write_private(&stage.join(HEAD), &fixture.head[..10]).unwrap(),
                "foreign bytes" => {
                    write_private(&stage.join(HEAD), b"unrelated owner bytes").unwrap()
                }
                "foreign name" => write_private(&stage.join("owner-file"), b"owner bytes").unwrap(),
                "symlink" => {
                    symlink(publisher_release_head_path(&fixture.data), stage.join(HEAD)).unwrap()
                }
                _ => unreachable!(),
            }
        }
        let result = fixture.load();
        if matches!(case, "complete" | "partial" | "empty destination") {
            assert_eq!(result.unwrap().release, fixture.release, "{case}");
            assert!(!fixture.data.join(MIGRATION).exists(), "{case}");
        } else {
            assert!(result.is_err(), "{case}");
            assert!(fixture.data.join(MIGRATION).exists(), "{case}");
            assert!(!fixture.data.join("installation").exists(), "{case}");
        }
    }
}

#[test]
fn guard_identity_and_pending_journal_precede_migration() {
    let fixture = Fixture::new();
    let guard = InstallationGuard::acquire(fixture.binary.parent().unwrap()).unwrap();
    assert!(fixture.load().is_err());
    fs::write(
        fixture
            .binary
            .parent()
            .unwrap()
            .join(".elastos.update-journal.json"),
        b"malformed pending journal",
    )
    .unwrap();
    assert!(load_or_migrate(&fixture.data, &fixture.binary, &fixture.source, &guard).is_err());
    assert!(!fixture.data.join("installation").exists());
    assert!(!fixture.data.join(MIGRATION).exists());
}

#[test]
fn signed_standalone_support_bytes_and_regular_file_are_required() {
    for case in [
        "valid",
        "changed",
        "missing",
        "directory",
        "symlink",
        "hardlink",
    ] {
        let mut fixture = Fixture::new();
        let bytes = b"installed signed kubo";
        let path = fixture.data.join("bin/kubo");
        fs::create_dir(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let components = json!({"schema":"elastos.components/v1", "external":{"kubo":{
            "version":"fixture", "install_path":"bin/kubo", "platforms":{(crate::setup::detect_platform()):{
                "strategy":"release", "install_path":"bin/kubo", "cid":raw_cid(bytes), "checksum":format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
            }}
        }}, "capsules":{}, "profiles":{"home":{"components":["kubo"]}}});
        fs::write(
            fixture.data.join("components.json"),
            serde_json::to_vec(&components).unwrap(),
        )
        .unwrap();
        fixture.bind_components();
        match case {
            "valid" => {}
            "changed" => fs::write(&path, b"substituted kubo").unwrap(),
            "missing" => fs::remove_file(&path).unwrap(),
            "directory" => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            "symlink" => {
                fs::remove_file(&path).unwrap();
                symlink(&fixture.binary, &path).unwrap();
            }
            "hardlink" => {
                fs::remove_file(&path).unwrap();
                fs::hard_link(&fixture.binary, &path).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(fixture.load().is_ok(), case == "valid", "{case}");
    }
}

#[test]
fn installed_catalogue_keeps_pinned_custody_after_offer_expiry() {
    for case in [
        "expired valid",
        "wrong signer",
        "wrong cid",
        "invalid expiry ordering",
    ] {
        let mut fixture = Fixture::new();
        let mut payload = crate::api::capsule_inventory::tests::model_catalog_fixture();
        payload["expires_at"] = json!(2);
        if case == "invalid expiry ordering" {
            payload["expires_at"] = json!(1);
        }
        let catalogue = signed(
            payload,
            "elastos.model.catalog.v1",
            if case == "wrong signer" { 72 } else { 71 },
        );
        let mut cid = raw_cid(&catalogue);
        if case == "wrong cid" {
            cid = raw_cid(b"different signed catalogue");
        }
        let components = json!({"schema":"elastos.components/v1", "external":{}, "capsules":{}, "profiles":{},
            "model_catalog":{"head_cid":cid, "publisher_dids":[crate::crypto::encode_signing_key_did(&key(71))]}});
        fs::write(
            fixture.data.join("components.json"),
            serde_json::to_vec(&components).unwrap(),
        )
        .unwrap();
        fs::write(fixture.data.join("model-catalog.json"), catalogue).unwrap();
        fixture.bind_components();
        assert_eq!(fixture.load().is_ok(), case == "expired valid", "{case}");
        // Current model discovery retains the real-time expiry gate.
        assert!(
            crate::api::capsule_inventory::model_catalog_entries(&fixture.data).is_err(),
            "{case}"
        );
    }
}

#[test]
fn chunked_components_descriptor_uses_signed_checksum_and_size() {
    let mut fixture = Fixture::new();
    let padding = "x".repeat(300 * 1024);
    let components = json!({"schema":"elastos.components/v1", "external":{}, "capsules":{}, "profiles":{}, "description":padding});
    fs::write(
        fixture.data.join("components.json"),
        serde_json::to_vec(&components).unwrap(),
    )
    .unwrap();
    fixture.bind_components();
    let mut release: Value = serde_json::from_slice(&fixture.release).unwrap();
    // A signed chunked UnixFS descriptor remains authenticated by checksum/size.
    release["payload"]["platforms"][crate::update::detect_release_platform()]["components"]
        ["cid"] = json!(cid::Cid::new_v1(
        0x70,
        cid::multihash::Multihash::<64>::wrap(0x12, &Sha256::digest(b"chunked root fixture"))
            .unwrap()
    )
    .to_string());
    fixture.release = signed(release["payload"].take(), "elastos.release.v1", 71);
    let mut head: Value = serde_json::from_slice(&fixture.head).unwrap();
    head["payload"]["latest_release_cid"] = json!(raw_cid(&fixture.release));
    head["payload"]["release_sha256"] = json!(hex::encode(Sha256::digest(&fixture.release)));
    fixture.head = signed(head["payload"].take(), "elastos.release.head.v1", 71);
    fixture.source.head_cid = raw_cid(&fixture.head);
    fixture.publish();
    assert!(fixture.load().is_ok());
    fs::write(fixture.data.join("components.json"), b"foreign components").unwrap();
    assert!(fixture.load().is_err());
}
