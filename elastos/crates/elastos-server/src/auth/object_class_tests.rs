//! Principal-root object classifier (#444, design §3 and §11.1 U1-U6).

use super::*;

struct Fixture {
    dir: tempfile::TempDir,
    principal_id: String,
    localhost_root: String,
    protection: PrincipalRootProtectionV1,
    data_key: [u8; 32],
}

impl Fixture {
    fn new(principal_id: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let protection = store_test_principal_root_protection(dir.path(), principal_id);
        let data_key = principal_root_data_key_from_protection(dir.path(), &protection).unwrap();
        Self {
            dir,
            principal_id: principal_id.to_string(),
            localhost_root: protection.localhost_root.clone(),
            protection,
            data_key,
        }
    }

    fn uri(&self, relative: &str) -> String {
        format!("{}/{relative}", self.localhost_root)
    }

    fn path(&self, uri: &str) -> PathBuf {
        rooted_localhost_fs_path(self.dir.path(), uri).unwrap()
    }

    fn envelope(&self, uri: &str, plaintext: &[u8]) -> Vec<u8> {
        encrypt_principal_root_object_bytes(
            &self.principal_id,
            &self.localhost_root,
            uri,
            &self.protection,
            &self.data_key,
            plaintext,
        )
        .unwrap()
    }

    fn envelope_struct(&self, uri: &str, plaintext: &[u8]) -> PrincipalRootObjectEnvelopeV1 {
        serde_json::from_slice(&self.envelope(uri, plaintext)).unwrap()
    }

    fn expected<'a>(&'a self, uri: &'a str) -> PrincipalRootObjectExpectation<'a> {
        PrincipalRootObjectExpectation {
            principal_id: &self.principal_id,
            localhost_root: &self.localhost_root,
            object_uri: uri,
            data_key_id: Some(&self.protection.data_key_id),
        }
    }

    fn classify(&self, uri: &str, bytes: &[u8]) -> PrincipalRootObjectClass {
        classify_principal_root_object_bytes(bytes, &self.expected(uri))
    }

    fn classify_file(&self, uri: &str, bytes: &[u8]) -> PrincipalRootObjectFileProbe {
        let path = self.dir.path().join("probe.bin");
        std::fs::write(&path, bytes).unwrap();
        let mut budget = MAX_MIGRATION_TOTAL_BYTES;
        self.classify_path(uri, &path, &mut budget)
    }

    fn classify_path(
        &self,
        uri: &str,
        path: &Path,
        budget: &mut u64,
    ) -> PrincipalRootObjectFileProbe {
        let (mut file, len) = open_principal_root_object_file(path).unwrap();
        classify_principal_root_object_file(&mut file, len, &self.expected(uri), budget).unwrap()
    }
}

fn label_unreadable() -> PrincipalRootObjectClass {
    PrincipalRootObjectClass::Damaged(PrincipalRootObjectDamage::LabelUnreadable)
}

fn file_label_unreadable() -> PrincipalRootObjectFileClass {
    PrincipalRootObjectFileClass::Damaged(PrincipalRootObjectDamage::LabelUnreadable)
}

fn canonical_head_len(envelope: &[u8]) -> usize {
    let marker = b"\"ciphertext\": \"";
    memchr::memmem::find(envelope, marker).unwrap() + marker.len()
}

fn sorted_key_envelope(envelope: &PrincipalRootObjectEnvelopeV1) -> Vec<u8> {
    let value = serde_json::to_value(envelope).unwrap();
    let sorted: std::collections::BTreeMap<String, Value> = value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    serde_json::to_vec_pretty(&sorted).unwrap()
}

fn ciphertext_first_envelope(envelope: &PrincipalRootObjectEnvelopeV1) -> Vec<u8> {
    format!(
        "{{\"ciphertext\":\"{}\",\"schema\":\"{}\",\"principal_id\":\"{}\",\"localhost_root\":\"{}\",\
         \"data_key_id\":\"{}\",\"object_uri\":\"{}\",\"cipher\":\"{}\",\"nonce\":\"{}\"}}",
        envelope.ciphertext,
        envelope.schema,
        envelope.principal_id,
        envelope.localhost_root,
        envelope.data_key_id,
        envelope.object_uri,
        envelope.cipher,
        envelope.nonce,
    )
    .into_bytes()
}

#[test]
fn classifier_accepts_canonical_envelopes_only() {
    let fixture = Fixture::new("person:local:classifier-canonical");
    let uri = fixture.uri("Documents/a.bin");
    let mut large = vec![0x5au8; 1024 * 1024 + 7];
    large[17] = 0;
    for plaintext in [&b""[..], &b"x"[..], &large[..]] {
        let envelope = fixture.envelope(&uri, plaintext);
        assert_eq!(
            fixture.classify(&uri, &envelope),
            PrincipalRootObjectClass::Envelope
        );
        let probe = fixture.classify_file(&uri, &envelope);
        assert_eq!(probe.class, PrincipalRootObjectFileClass::Envelope);
        assert!(
            probe.bytes_read <= (HEAD_WINDOW_FIRST + CANONICAL_TAIL.len()) as u64,
            "an envelope costs at most 4 KiB plus a 3-byte tail, read {}",
            probe.bytes_read
        );
    }

    // A head longer than the first window grows it, continuing, never re-reading.
    let long_uri = fixture.uri(&format!("Documents/{}", "d/".repeat(2_600)));
    let envelope = fixture.envelope(&long_uri, b"long head");
    assert!(canonical_head_len(&envelope) > HEAD_WINDOW_FIRST);
    let probe = fixture.classify_file(&long_uri, &envelope);
    assert_eq!(probe.class, PrincipalRootObjectFileClass::Envelope);
    assert_eq!(probe.bytes_read, probe.prefix.len() as u64);
    assert!(probe.prefix.len() <= 2 * HEAD_WINDOW_FIRST);
    assert_eq!(&envelope[..probe.prefix.len()], &probe.prefix[..]);
    let big = fixture.envelope(&long_uri, &large);
    let probe = fixture.classify_file(&long_uri, &big);
    assert_eq!(probe.class, PrincipalRootObjectFileClass::Envelope);
    assert_eq!(
        probe.bytes_read,
        probe.prefix.len() as u64 + CANONICAL_TAIL.len() as u64
    );
    assert!(probe.prefix.len() <= 2 * HEAD_WINDOW_FIRST);

    // Valid but non-canonical layouts are not Envelope.
    let envelope = fixture.envelope_struct(&uri, b"layout");
    for layout in [
        serde_json::to_vec(&envelope).unwrap(),
        sorted_key_envelope(&envelope),
    ] {
        assert_eq!(fixture.classify(&uri, &layout), label_unreadable());
        assert_eq!(
            fixture.classify_file(&uri, &layout).class,
            file_label_unreadable()
        );
    }
}

#[test]
fn classifier_never_calls_canonical_damage_plaintext() {
    let fixture = Fixture::new("person:local:classifier-damage");
    let uri = fixture.uri("Documents/damage.txt");
    let envelope = fixture.envelope(&uri, b"canonical body for damage tests");
    let head_len = canonical_head_len(&envelope);
    let damaged = |bytes: &[u8], what: &str| {
        assert!(
            matches!(
                fixture.classify(&uri, bytes),
                PrincipalRootObjectClass::Damaged(_)
            ),
            "{what} must be Damaged"
        );
    };

    for index in 0..head_len {
        let mut bytes = envelope.clone();
        bytes[index] = 0xff;
        damaged(&bytes, &format!("0xff at {index}"));
        assert!(
            matches!(
                fixture.classify_file(&uri, &bytes).class,
                PrincipalRootObjectFileClass::Damaged(_)
            ),
            "file with 0xff at {index} must be Damaged"
        );
    }
    for first in 0..head_len {
        for second in first + 1..head_len {
            let mut bytes = envelope.clone();
            bytes[first] = 0xff;
            bytes[second] = 0xff;
            damaged(&bytes, &format!("0xff at {first} and {second}"));
        }
    }
    for start in 0..=head_len - 5 {
        let mut bytes = envelope.clone();
        bytes[start..start + 5].fill(0xff);
        damaged(&bytes, &format!("5-byte burst at {start}"));
    }
    for len in 1..=(head_len + 64).min(envelope.len() - 1) {
        damaged(&envelope[..len], &format!("truncated to {len}"));
        assert_eq!(
            fixture.classify_file(&uri, &envelope[..len]).class,
            file_label_unreadable(),
            "file truncated to {len}"
        );
    }
    // A cut in the body.
    let cut = &envelope[..envelope.len() - 10];
    damaged(cut, "cut in the body");

    // `{\n  "sche` + 0xff, alone and followed by the rest of the header.
    let mut sche = b"{\n  \"sche".to_vec();
    sche.push(0xff);
    damaged(&sche, "sche + 0xff alone");
    let mut rest = envelope.clone();
    rest[9] = 0xff;
    damaged(&rest, "sche + 0xff with the rest");
}

#[test]
fn classifier_damaged_noncanonical_shapes() {
    let fixture = Fixture::new("person:local:classifier-noncanonical");
    let uri = fixture.uri("Documents/shapes.json");
    let envelope = fixture.envelope_struct(&uri, &vec![7u8; 4096]);

    let ciphertext_first = ciphertext_first_envelope(&envelope);
    let truncated = &ciphertext_first[..ciphertext_first.len() / 3];
    assert_eq!(fixture.classify(&uri, truncated), label_unreadable());
    assert_eq!(
        fixture.classify_file(&uri, truncated).class,
        file_label_unreadable()
    );

    let compact = serde_json::to_vec(&envelope).unwrap();
    let mut sche = compact.clone();
    assert_eq!(&sche[..6], b"{\"sche");
    sche[6] = 0xff;
    assert_eq!(fixture.classify(&uri, &sche), label_unreadable());
    assert_eq!(&sche[..7], b"{\"sche\xff");

    let malformed = format!(
        "{{\"schema\":\"{PRINCIPAL_ROOT_OBJECT_SCHEMA}\",\"data_key_id\":\"{}\",\"nonce\":\"AAAAAAAAAAAAAAAA\",\"ciphertext\":",
        fixture.protection.data_key_id
    )
    .into_bytes();
    assert_eq!(fixture.classify(&uri, &malformed), label_unreadable());
}

#[test]
fn classifier_never_plaintexts_a_parsable_envelope() {
    let fixture = Fixture::new("person:local:classifier-parsable");
    let uri = fixture.uri("Documents/parsable.bin");
    let canonical = fixture.envelope(&uri, b"parsable body");
    let envelope = fixture.envelope_struct(&uri, &vec![3u8; 100 * 1024]);
    let mut cases = Vec::new();
    let ciphertext_first = ciphertext_first_envelope(&envelope);
    assert!(ciphertext_first.len() > 128 * 1024);
    cases.push(("ciphertext first over 128 KiB", ciphertext_first));
    let mut trailing = canonical.clone();
    trailing.extend(std::iter::repeat_n(b' ', 70 * 1024));
    cases.push(("70 KiB after the closing brace", trailing));
    let mut between = b"{".to_vec();
    between.extend(std::iter::repeat_n(b' ', 70 * 1024));
    between.extend_from_slice(&canonical[1..]);
    cases.push(("70 KiB between members", between));
    let mut leading = vec![b' '; 1024 * 1024 - canonical.len()];
    leading.extend_from_slice(&canonical);
    cases.push(("leading whitespace at 1 MiB", leading));
    for (what, bytes) in cases {
        assert!(principal_root_object_parses_as_envelope(&bytes), "{what}");
        assert_eq!(fixture.classify(&uri, &bytes), label_unreadable(), "{what}");
        assert_eq!(
            fixture.classify_file(&uri, &bytes).class,
            file_label_unreadable(),
            "{what}"
        );
    }
}

#[test]
fn classifier_leaves_a_padded_envelope_over_16_mib_unencrypted() {
    let fixture = Fixture::new("person:local:classifier-padded-17");
    let uri = fixture.uri("Documents/padded.json");
    let path = fixture.path(&uri);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let canonical = fixture.envelope(&uri, b"padded body");
    let mut padded = vec![b' '; 17 * 1024 * 1024];
    padded.extend_from_slice(&canonical);
    std::fs::write(&path, &padded).unwrap();

    let mut budget = MAX_MIGRATION_TOTAL_BYTES;
    let probe = fixture.classify_path(&uri, &path, &mut budget);
    assert_eq!(
        probe.class,
        PrincipalRootObjectFileClass::UnconfirmedPlaintext
    );
    assert_eq!(probe.bytes_read, HEAD_WINDOW_MAX as u64);
    assert_eq!(budget, MAX_MIGRATION_TOTAL_BYTES);

    // Opening reports it as not encrypted without reading it whole, and lazy
    // protection refuses it by name before reading.
    let err = read_principal_root_object(
        fixture.dir.path(),
        &fixture.principal_id,
        &fixture.localhost_root,
        &uri,
        &path,
    )
    .unwrap_err();
    assert!(err
        .to_string()
        .contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED));
    let err = protect_legacy_plaintext_principal_root_object(
        fixture.dir.path(),
        &fixture.principal_id,
        &fixture.localhost_root,
        &uri,
        &path,
    )
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("Documents/padded.json is not encrypted and is larger than 16 MiB"));
    // Migration prepare refuses it before reading.
    let plan = PrincipalRootMigrationPlanV1 {
        schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
        principal_id: fixture.principal_id.clone(),
        localhost_root: fixture.localhost_root.clone(),
        objects: vec![PrincipalRootMigrationSelectionV1 {
            object_uri: uri.clone(),
            plaintext_sha256: sha256_label(&padded),
        }],
    };
    let backup_dir = fixture.dir.path().join("backups").join("padded");
    std::fs::create_dir_all(backup_dir.parent().unwrap()).unwrap();
    assert!(migrate_principal_root_objects_plan_locked(
        fixture.dir.path(),
        &plan,
        &sha256_label(b"padded plan"),
        &backup_dir,
    )
    .is_err());
    assert!(!backup_dir.exists());
    assert_eq!(std::fs::read(&path).unwrap(), padded);
}

#[test]
fn classifier_keeps_runtime_and_user_plaintext_plaintext() {
    let fixture = Fixture::new("person:local:classifier-plaintext");
    let uri = fixture.uri("Documents/plain");
    let transaction_store = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": "elastos.runtime.transaction-effect-store/v1",
        "effects": [{
            "intent": {"chain_id": 1, "nonce": "0x7", "to": "0x00", "value": "0x0"},
            "status": "pending",
        }],
    }))
    .unwrap();
    let home_state = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": "elastos.home.browser-state/v1",
        "tabs": [],
    }))
    .unwrap();
    let mut random = Vec::new();
    let mut block = Sha256::digest(b"classifier random seed");
    while random.len() < 64 * 1024 {
        random.extend_from_slice(&block);
        block = Sha256::digest(block);
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0u8; 512]);
    let mut mp4 = b"\0\0\0\x18ftypmp42".to_vec();
    mp4.extend_from_slice(&[1u8; 512]);
    let plaintexts: Vec<(&str, Vec<u8>)> = vec![
        ("transaction-effect store", transaction_store),
        ("Home browser state", home_state),
        ("random binary", random),
        ("PNG", png),
        ("MP4", mp4),
        ("empty", Vec::new()),
        ("{}", b"{}".to_vec()),
        ("short JSON", b"{\n  \"a\": 1\n}".to_vec()),
        ("only nonce", br#"{"nonce": "0x1", "amount": 3}"#.to_vec()),
        (
            "JSON Schema",
            br#"{"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object"}"#
                .to_vec(),
        ),
    ];
    for (what, bytes) in plaintexts {
        assert_eq!(
            fixture.classify(&uri, &bytes),
            PrincipalRootObjectClass::Plaintext,
            "{what}"
        );
        assert_eq!(
            fixture.classify_file(&uri, &bytes).class,
            PrincipalRootObjectFileClass::Plaintext,
            "{what}"
        );
    }

    let lookalike =
        b"{\n  \"schema\": \"elastos.principal-root.object/notes\",\n  \"mine\": true\n}";
    assert_eq!(fixture.classify(&uri, lookalike), label_unreadable());
    let notice = PrincipalRootObjectNotice::new(
        fixture.dir.path(),
        &fixture.localhost_root,
        &uri,
        PrincipalRootObjectDamage::LabelUnreadable,
    );
    assert_eq!(notice.path, "Documents/plain");
    assert!(notice.to_string().starts_with(
        "Documents/plain does not carry the label this Home writes on encrypted files"
    ));
    let jwe = br#"{"protected":"eyJhbGciOiJBMjU2S1cifQ","iv":"AxY8DCtDaGlsbGljb3RoZQ","ciphertext":"KDlTtXchhZTGufMYmOYGS4HffxPSUrfmqCHXaI9wOGY","tag":"Mz-VPPyU4RlcuYv1IwIvzw"}"#;
    assert_eq!(fixture.classify(&uri, jwe), label_unreadable());
}

#[test]
fn classifier_binding_reasons() {
    let fixture = Fixture::new("person:local:classifier-binding");
    let uri = fixture.uri("Documents/bound.txt");
    let envelope = fixture.envelope(&uri, b"bound body");
    let other_key = format!("pdek:{}", "0".repeat(32));
    let mut expected = fixture.expected(&uri);
    expected.data_key_id = Some(&other_key);
    assert_eq!(
        classify_principal_root_object_bytes(&envelope, &expected),
        PrincipalRootObjectClass::Damaged(PrincipalRootObjectDamage::OtherKey {
            file_key_id: fixture.protection.data_key_id.clone(),
            current_key_id: other_key.clone(),
        })
    );
    let other_uri = fixture.uri("Documents/elsewhere.txt");
    assert_eq!(
        fixture.classify(&other_uri, &envelope),
        PrincipalRootObjectClass::Damaged(PrincipalRootObjectDamage::WrongPlace {
            label_uri: uri.clone()
        })
    );
    let mut expected = fixture.expected(&uri);
    expected.principal_id = "person:local:someone-else";
    assert_eq!(
        classify_principal_root_object_bytes(&envelope, &expected),
        PrincipalRootObjectClass::Damaged(PrincipalRootObjectDamage::OtherHome)
    );
    let mut expected = fixture.expected(&uri);
    expected.data_key_id = None;
    assert_eq!(
        classify_principal_root_object_bytes(&envelope, &expected),
        PrincipalRootObjectClass::Damaged(PrincipalRootObjectDamage::NoKey)
    );

    let mut v2 = fixture.envelope_struct(&uri, b"bound body");
    v2.schema = "elastos.principal-root.object/v2".to_string();
    let v2 = serde_json::to_vec_pretty(&v2).unwrap();
    assert_eq!(
        fixture.classify(&uri, &v2),
        PrincipalRootObjectClass::Damaged(PrincipalRootObjectDamage::UnsupportedFormat {
            schema: "elastos.principal-root.object/v2".to_string(),
            cipher: DEFAULT_PRINCIPAL_ROOT_CIPHER.to_string(),
        })
    );

    let rewrite = |edit: &dyn Fn(&mut PrincipalRootObjectEnvelopeV1)| {
        let mut envelope = fixture.envelope_struct(&uri, b"bound body");
        edit(&mut envelope);
        serde_json::to_vec_pretty(&envelope).unwrap()
    };
    let short = rewrite(&|envelope| envelope.ciphertext = "A".repeat(21));
    assert_eq!(fixture.classify(&uri, &short), label_unreadable());
    let mod_one = rewrite(&|envelope| {
        while envelope.ciphertext.len() % 4 != 1 {
            envelope.ciphertext.push('A');
        }
    });
    assert_eq!(fixture.classify(&uri, &mod_one), label_unreadable());
    let nonce = rewrite(&|envelope| envelope.nonce = b64_url(&[0u8; 8]));
    assert_eq!(fixture.classify(&uri, &nonce), label_unreadable());
}

#[test]
fn read_path_blocks_damage_and_opens_authentic_noncanonical_envelopes() {
    let fixture = Fixture::new("person:local:classifier-read");
    let uri = fixture.uri("Documents/read.txt");
    let path = fixture.path(&uri);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let read = || {
        read_principal_root_object(
            fixture.dir.path(),
            &fixture.principal_id,
            &fixture.localhost_root,
            &uri,
            &path,
        )
    };

    let mut flipped = fixture.envelope_struct(&uri, b"authentic body");
    let replacement = if flipped.ciphertext.starts_with('A') {
        "B"
    } else {
        "A"
    };
    flipped.ciphertext.replace_range(..1, replacement);
    let flipped = serde_json::to_vec_pretty(&flipped).unwrap();
    std::fs::write(&path, &flipped).unwrap();
    let message = read().unwrap_err().to_string();
    assert!(message.contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_DAMAGED));
    assert!(
        message.contains("Documents/read.txt is damaged: its contents failed the integrity check")
    );
    assert_eq!(std::fs::read(&path).unwrap(), flipped);

    let mut header = fixture.envelope(&uri, b"authentic body");
    header[9] = 0xff;
    std::fs::write(&path, &header).unwrap();
    let message = read().unwrap_err().to_string();
    assert!(message.contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_DAMAGED));
    assert!(!message.contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED));
    assert!(message.contains("does not carry the label"));
    assert!(protect_legacy_plaintext_principal_root_object(
        fixture.dir.path(),
        &fixture.principal_id,
        &fixture.localhost_root,
        &uri,
        &path,
    )
    .unwrap_err()
    .to_string()
    .contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_DAMAGED));
    assert_eq!(std::fs::read(&path).unwrap(), header);

    let compact = serde_json::to_vec(&fixture.envelope_struct(&uri, b"compact body")).unwrap();
    std::fs::write(&path, &compact).unwrap();
    assert_eq!(read().unwrap(), b"compact body");

    let mut padded = fixture.envelope(&uri, b"padded body");
    padded.extend_from_slice(b"\n\n");
    std::fs::write(&path, &padded).unwrap();
    assert_eq!(read().unwrap(), b"padded body");

    // #450 L9: an authentic envelope in another layout opens at any size,
    // here padded past the largest envelope an upload writes (about 134 MiB).
    padded.resize(135 * 1024 * 1024, b'\n');
    std::fs::write(&path, &padded).unwrap();
    drop(padded);
    assert_eq!(read().unwrap(), b"padded body");

    std::fs::write(&path, b"legacy plaintext").unwrap();
    assert!(read()
        .unwrap_err()
        .to_string()
        .contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED));
    assert_eq!(
        protect_legacy_plaintext_principal_root_object(
            fixture.dir.path(),
            &fixture.principal_id,
            &fixture.localhost_root,
            &uri,
            &path,
        )
        .unwrap(),
        b"legacy plaintext"
    );
    assert_eq!(read().unwrap(), b"legacy plaintext");
}

/// #450 L9: a file whose canonical label names another key is refused with
/// its notice without reading past the label, however large it is: here a
/// 1 TiB sparse file that no full read could finish.
#[test]
fn open_refuses_a_foreign_label_without_reading_the_body() {
    let fixture = Fixture::new("person:local:classifier-foreign-large");
    let uri = fixture.uri("Documents/foreign.bin");
    let path = fixture.path(&uri);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut foreign = fixture.envelope_struct(&uri, b"foreign body");
    foreign.data_key_id = "pdek:another-home-key".to_string();
    let canonical = serde_json::to_vec_pretty(&foreign).unwrap();
    let head = &canonical[..canonical_head_len(&canonical)];
    // A decodable ciphertext length keeps the canonical frame valid.
    let len = (head.len() + CANONICAL_TAIL.len()) as u64 + 4 * (1u64 << 38);
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(head).unwrap();
        file.set_len(len).unwrap();
        file.seek(SeekFrom::Start(len - CANONICAL_TAIL.len() as u64))
            .unwrap();
        file.write_all(CANONICAL_TAIL).unwrap();
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let (data_dir, principal_id, localhost_root) = (
        fixture.dir.path().to_path_buf(),
        fixture.principal_id.clone(),
        fixture.localhost_root.clone(),
    );
    let (read_uri, read_path) = (uri.clone(), path.clone());
    std::thread::spawn(move || {
        let read = read_principal_root_object(
            &data_dir,
            &principal_id,
            &localhost_root,
            &read_uri,
            &read_path,
        );
        let _ = sender.send(read.map_err(|err| err.to_string()));
    });
    let message = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("a foreign label was read past its head")
        .unwrap_err();
    assert!(
        message.contains(PROTECTED_PRINCIPAL_ROOT_OBJECT_DAMAGED),
        "{message}"
    );
    assert!(
        message.contains("is labelled for key pdek:another-home-key"),
        "{message}"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
}

/// #450 L8: a FIFO swapped in for a protected object after the walk must not
/// block the open, which runs while the caller holds its locks.
#[cfg(unix)]
#[test]
fn open_refuses_a_fifo_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new("person:local:classifier-fifo");
    let uri = fixture.uri("Documents/swapped.txt");
    let path = fixture.path(&uri);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(open_principal_root_object_file(&path).map(|_| ()));
    });
    let opened = receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("opening a FIFO blocked");
    assert!(opened
        .unwrap_err()
        .to_string()
        .contains("must be a regular file"));
}

#[test]
fn migration_prepare_refuses_bytes_swapped_after_preflight() {
    let fixture = Fixture::new("person:local:classifier-swap");
    let uri = fixture.uri("Documents/swap.txt");
    let path = fixture.path(&uri);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"legacy plaintext").unwrap();
    let plan = PrincipalRootMigrationPlanV1 {
        schema: PRINCIPAL_ROOT_MIGRATION_PLAN_SCHEMA.to_string(),
        principal_id: fixture.principal_id.clone(),
        localhost_root: fixture.localhost_root.clone(),
        objects: vec![PrincipalRootMigrationSelectionV1 {
            object_uri: uri.clone(),
            plaintext_sha256: sha256_label(b"legacy plaintext"),
        }],
    };
    let mut damaged = fixture.envelope(&uri, b"legacy plaintext");
    damaged[9] = 0xff;
    inject_principal_root_prepare_swap(fixture.dir.path(), &uri, damaged.clone());
    let backup_dir = fixture.dir.path().join("backups").join("swap");
    std::fs::create_dir_all(backup_dir.parent().unwrap()).unwrap();
    let err = migrate_principal_root_objects_plan_locked(
        fixture.dir.path(),
        &plan,
        &sha256_label(b"swap plan"),
        &backup_dir,
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Documents/swap.txt changed while Home was preparing to encrypt it. Nothing was changed; try again."
    );
    assert!(!backup_dir.exists());
    assert!(!principal_root_migration_journal_path(fixture.dir.path())
        .unwrap()
        .exists());
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
}
