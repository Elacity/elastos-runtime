//! Header-only readiness, plaintext-only migration limits and named
//! refusals (#444, design §4-§5 and §11.1 U7-U11, U20).

use super::*;

const MIB: usize = 1024 * 1024;

struct Home {
    dir: tempfile::TempDir,
    principal_id: String,
    localhost_root: String,
    protection: PrincipalRootProtectionV1,
    data_key: [u8; 32],
}

impl Home {
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

    fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    fn uri(&self, relative: &str) -> String {
        format!("{}/{relative}", self.localhost_root)
    }

    fn path(&self, relative: &str) -> PathBuf {
        let path = rooted_localhost_fs_path(self.data_dir(), &self.uri(relative)).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    }

    fn envelope(&self, relative: &str, plaintext: &[u8]) -> Vec<u8> {
        encrypt_principal_root_object_bytes(
            &self.principal_id,
            &self.localhost_root,
            &self.uri(relative),
            &self.protection,
            &self.data_key,
            plaintext,
        )
        .unwrap()
    }

    fn write_envelope(&self, relative: &str, plaintext: &[u8]) -> Vec<u8> {
        let envelope = self.envelope(relative, plaintext);
        std::fs::write(self.path(relative), &envelope).unwrap();
        envelope
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        std::fs::write(self.path(relative), bytes).unwrap();
    }

    /// A sparse file of zeros: no sign of an envelope anywhere.
    fn write_zeros(&self, relative: &str, len: usize) {
        std::fs::File::create(self.path(relative))
            .unwrap()
            .set_len(len as u64)
            .unwrap();
    }

    /// A sparse file of zeros whose only sign of an envelope sits at its
    /// end, past the 64 KiB head: Damaged only once read whole.
    fn write_late_sign(&self, relative: &str, len: usize) {
        use std::io::{Seek, SeekFrom, Write};
        let sign = b"\"ciphertext\"";
        let mut file = std::fs::File::create(self.path(relative)).unwrap();
        file.set_len(len as u64).unwrap();
        file.seek(SeekFrom::Start((len - sign.len()) as u64))
            .unwrap();
        file.write_all(sign).unwrap();
    }

    fn read(&self, relative: &str) -> Vec<u8> {
        std::fs::read(self.path(relative)).unwrap()
    }

    fn open(&self, relative: &str) -> anyhow::Result<Vec<u8>> {
        read_principal_root_object(
            self.data_dir(),
            &self.principal_id,
            &self.localhost_root,
            &self.uri(relative),
            &self.path(relative),
        )
    }

    fn declarations(&self) -> Vec<PrincipalRootUpgradeDeclarationV1> {
        vec![PrincipalRootUpgradeDeclarationV1 {
            principal_id: self.principal_id.clone(),
            localhost_root: self.localhost_root.clone(),
            inventory: ["Documents", "Pictures", ".Trash"]
                .into_iter()
                .map(|folder| PrincipalRootProtectedObjectDeclarationV1::root(self.uri(folder)))
                .collect(),
        }]
    }

    fn ready(&self) -> anyhow::Result<Vec<String>> {
        verify_declared_principal_roots_ready(self.data_dir(), &self.declarations())
    }

    fn backup_dir(&self) -> PathBuf {
        self.data_dir()
            .join("backups")
            .join(format!("principal-root-upgrade-{}", now_ts()))
    }

    fn upgrade(&self, backup_dir: &Path) -> anyhow::Result<PrincipalRootUpgradeReceiptV1> {
        let declarations = self.declarations();
        migrate_declared_principal_roots_offline(self.data_dir(), backup_dir, || Ok(declarations))
    }

    fn journal_exists(&self) -> bool {
        principal_root_migration_journal_path(self.data_dir())
            .unwrap()
            .exists()
    }

    fn probes(&self) -> PrincipalRootScanProbes {
        principal_root_scan_probes(self.data_dir())
    }
}

fn probe_delta(
    before: PrincipalRootScanProbes,
    after: PrincipalRootScanProbes,
) -> PrincipalRootScanProbes {
    PrincipalRootScanProbes {
        scans: after.scans - before.scans,
        file_opens: after.file_opens - before.file_opens,
        bytes_read: after.bytes_read - before.bytes_read,
    }
}

fn header_budget(files: usize) -> u64 {
    (files * (HEAD_WINDOW_FIRST + CANONICAL_TAIL.len())) as u64
}

#[test]
fn start_ready_with_100_mib_envelope_over_64_mib_total_and_1100_files() {
    let home = Home::new("person:local:scan-large-home");
    home.write_envelope("Documents/film.mov", &vec![0x11; 100 * MIB]);
    for index in 0..5 {
        home.write_envelope(&format!("Pictures/raw-{index}.dng"), &vec![0x22; 15 * MIB]);
    }
    for index in 0..1_100 {
        let folder = ["Documents/notes", "Pictures/thumbs", ".Trash"][index % 3];
        home.write_envelope(&format!("{folder}/{index:04}.txt"), b"small");
    }
    let before = home.probes();
    assert_eq!(home.ready().unwrap(), Vec::<String>::new());
    let probes = probe_delta(before, home.probes());
    assert_eq!(probes.scans, 1);
    assert_eq!(probes.file_opens, 1_106);
    // No body is read, so nothing can be decrypted: every file costs its head.
    assert!(
        probes.bytes_read <= header_budget(1_106),
        "read {} bytes",
        probes.bytes_read
    );
}

#[test]
fn start_continues_with_damaged_and_names_it() {
    let home = Home::new("person:local:scan-damaged-home");
    let mut damaged = home.envelope("Documents/damaged.txt", b"damaged body");
    damaged[9] = 0xff;
    home.write("Documents/damaged.txt", &damaged);
    let other_dir = tempfile::tempdir().unwrap();
    let other = store_test_principal_root_protection(other_dir.path(), "person:local:other-key");
    let other_key = principal_root_data_key_from_protection(other_dir.path(), &other).unwrap();
    let other_key_envelope = encrypt_principal_root_object_bytes(
        &home.principal_id,
        &home.localhost_root,
        &home.uri("Documents/other-key.txt"),
        &other,
        &other_key,
        b"other key body",
    )
    .unwrap();
    home.write("Documents/other-key.txt", &other_key_envelope);
    home.write_envelope("Documents/fine.txt", b"fine");

    let notices = home.ready().unwrap();
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].starts_with(
        "Documents/damaged.txt does not carry the label this Home writes on encrypted files"
    ));
    assert!(notices[1].starts_with(&format!(
        "Documents/other-key.txt is labelled for key {}, but this Home's key is {}, so Home will \
         not open it.",
        other.data_key_id, home.protection.data_key_id
    )));
    assert_eq!(home.read("Documents/damaged.txt"), damaged);
    assert_eq!(home.read("Documents/other-key.txt"), other_key_envelope);
}

#[test]
fn start_refuses_plaintext_with_names() {
    let home = Home::new("person:local:scan-plaintext-home");
    for name in ["a", "b", "c", "d", "e"] {
        home.write(&format!("Documents/{name}.txt"), b"legacy plaintext");
    }
    let error = home.ready().unwrap_err();
    let outcome = error
        .downcast_ref::<PrincipalRootMigrationRequiredV1>()
        .expect("start refuses with the migration-required outcome");
    let message = outcome.to_string();
    assert!(
        message.starts_with(
            "5 file(s) in protected Home folders are not encrypted, including Documents/a.txt, \
             Documents/b.txt, Documents/c.txt. Stop Home, then run: \""
        ),
        "{message}"
    );
    assert!(message.contains(&format!(
        "\" principal-root-upgrade --data-dir \"{}\" --backup-dir \"{}/backups/principal-root-upgrade-",
        home.data_dir().display(),
        home.data_dir().display()
    )));
    // The HTTP 409 body is unchanged: the names and command stay out of it.
    assert_eq!(
        serde_json::to_value(outcome).unwrap(),
        serde_json::json!({
            "schema": PRINCIPAL_ROOT_MIGRATION_REQUIRED_SCHEMA,
            "status": "migration_required",
            "code": "declared_plaintext_objects",
            "principal_id": home.principal_id,
            "localhost_root": home.localhost_root,
            "plaintext_object_count": 5,
        })
    );

    let many = Home::new("person:local:scan-seventy-home");
    for index in 0..70 {
        many.write(&format!("Documents/{index:02}.txt"), b"legacy");
    }
    let error = many.ready().unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<PrincipalRootMigrationRequiredV1>()
            .unwrap()
            .plaintext_object_count,
        70
    );

    // Markerless candidates are read whole only within the 80 MiB probe
    // cap; those read are not encrypted and start still refuses.
    let large = Home::new("person:local:scan-budget-home");
    for index in 0..6 {
        large.write_zeros(&format!("Documents/blank-{index}.bin"), 15 * MIB);
    }
    let before = large.probes();
    let error = large.ready().unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<PrincipalRootMigrationRequiredV1>()
            .unwrap()
            .plaintext_object_count,
        5
    );
    let probes = probe_delta(before, large.probes());
    assert!(
        probes.bytes_read <= MAX_PRINCIPAL_ROOT_SCAN_PROBE_BYTES + 6 * HEAD_WINDOW_MAX as u64,
        "read {} bytes",
        probes.bytes_read
    );
}

#[test]
fn migration_refusals_name_the_file_before_any_write() {
    let assert_untouched = |home: &Home, backup_dir: &Path| {
        assert!(!home.journal_exists());
        assert!(!backup_dir.exists());
    };

    let home = Home::new("person:local:scan-p2-home");
    home.write_zeros("Documents/big.bin", 17 * MIB);
    let backup_dir = home.backup_dir();
    let message = home.upgrade(&backup_dir).unwrap_err().to_string();
    assert!(
        message.starts_with(
            "Documents/big.bin is not encrypted and is 17 MiB. Home encrypts existing files of \
             up to 16 MiB in place. Move it out of "
        ),
        "{message}"
    );
    assert!(message.ends_with(
        " and try again; once Home is running, upload it again in Library, which encrypts files \
         of up to 100 MiB."
    ));
    assert_untouched(&home, &backup_dir);
    assert_eq!(
        std::fs::metadata(home.path("Documents/big.bin"))
            .unwrap()
            .len(),
        17 * MIB as u64
    );

    let home = Home::new("person:local:scan-p3-home");
    for index in 0..5 {
        home.write_zeros(&format!("Documents/part-{index}.bin"), 15 * MIB);
    }
    let backup_dir = home.backup_dir();
    assert_eq!(
        home.upgrade(&backup_dir).unwrap_err().to_string(),
        "Unencrypted files in protected Home folders add up to more than 64 MiB (reached at \
         Documents/part-4.bin); Home encrypts up to 64 MiB in one run. Move some of them out of \
         their folders and try again; then move them back and try again."
    );
    assert_untouched(&home, &backup_dir);

    let home = Home::new("person:local:scan-p4-home");
    for index in 0..65 {
        home.write(&format!("Documents/{index:02}.txt"), b"legacy");
    }
    let backup_dir = home.backup_dir();
    assert_eq!(
        home.upgrade(&backup_dir).unwrap_err().to_string(),
        "65 files in protected Home folders are not encrypted (first: Documents/00.txt); Home \
         encrypts up to 64 in one run. Move some of them out of their folders and try again; \
         then move them back and try again."
    );
    assert_untouched(&home, &backup_dir);
    assert_eq!(home.read("Documents/00.txt"), b"legacy");

    // Envelopes count toward no limit.
    let home = Home::new("person:local:scan-mixed-home");
    home.write_envelope("Documents/film.mov", &vec![0x33; 100 * MIB]);
    for index in 0..10 {
        home.write(
            &format!("Pictures/{index}.txt"),
            format!("legacy {index}").as_bytes(),
        );
    }
    let receipt = home.upgrade(&home.backup_dir()).unwrap();
    assert_eq!(receipt.status, "migrated");
    assert_eq!(receipt.object_count, 10);
    for index in 0..10 {
        assert_eq!(
            home.open(&format!("Pictures/{index}.txt")).unwrap(),
            format!("legacy {index}").as_bytes()
        );
    }
    assert_eq!(home.ready().unwrap(), Vec::<String>::new());
}

#[test]
fn damaged_envelope_never_selected_or_rewrapped() {
    let home = Home::new("person:local:scan-never-rewrap-home");
    let mut damaged = home.envelope("Documents/damaged.txt", b"damaged body");
    damaged[9] = 0xff;
    home.write("Documents/damaged.txt", &damaged);
    home.write("Documents/legacy.txt", b"legacy plaintext");
    let receipt = home.upgrade(&home.backup_dir()).unwrap();
    assert_eq!(receipt.object_count, 1);
    assert_eq!(receipt.notices.len(), 1);
    assert!(receipt.notices[0].starts_with("Documents/damaged.txt does not carry the label"));
    assert_eq!(home.read("Documents/damaged.txt"), damaged);
    assert_eq!(
        home.open("Documents/legacy.txt").unwrap(),
        b"legacy plaintext"
    );

    // A selected file swapped for damaged bytes after preflight is refused
    // by prepare, before anything is written.
    let home = Home::new("person:local:scan-swap-home");
    home.write("Documents/legacy.txt", b"legacy plaintext");
    let mut swapped = home.envelope("Documents/legacy.txt", b"legacy plaintext");
    swapped[9] = 0xff;
    inject_principal_root_prepare_swap(
        home.data_dir(),
        &home.uri("Documents/legacy.txt"),
        swapped.clone(),
    );
    let backup_dir = home.backup_dir();
    assert_eq!(
        home.upgrade(&backup_dir).unwrap_err().to_string(),
        "Documents/legacy.txt changed while Home was preparing to encrypt it. Nothing was \
         changed; try again."
    );
    assert!(!home.journal_exists());
    assert!(!backup_dir.join("root-000").exists());
    assert_eq!(home.read("Documents/legacy.txt"), swapped);
}

#[test]
fn plaintext_classified_fragment_is_wrapped_byte_exact() {
    let home = Home::new("person:local:scan-fragment-home");
    let canonical = home.envelope("Documents/fragment.bin", b"fragment source");
    let mut fragment = canonical[..44].to_vec();
    fragment[0] = b'[';
    fragment[1] = b'\r';
    fragment[2] = b'\t';
    home.write("Documents/fragment.bin", &fragment);
    let mut header = home.envelope("Documents/header.txt", b"header body");
    header[9] = 0xff;
    home.write("Documents/header.txt", &header);
    let mut nonce = home.envelope("Documents/nonce.txt", b"nonce body");
    let nonce_at = memchr::memmem::find(&nonce, b"\"nonce\": \"").unwrap() + 10;
    nonce[nonce_at] = if nonce[nonce_at] == b'A' { b'B' } else { b'A' };
    home.write("Documents/nonce.txt", &nonce);

    // At start: the fragment is plaintext, the 0xff header is damaged, the
    // changed nonce keeps its label and is an envelope.
    let error = home.ready().unwrap_err();
    let outcome = error
        .downcast_ref::<PrincipalRootMigrationRequiredV1>()
        .unwrap();
    assert_eq!(outcome.plaintext_object_count, 1);
    assert_eq!(outcome.examples, vec!["Documents/fragment.bin".to_string()]);

    let backup_dir = home.backup_dir();
    let receipt = home.upgrade(&backup_dir).unwrap();
    assert_eq!(receipt.object_count, 1);
    assert_eq!(receipt.notices.len(), 1);
    assert!(receipt.notices[0].starts_with("Documents/header.txt "));
    assert_eq!(home.open("Documents/fragment.bin").unwrap(), fragment);
    assert_eq!(home.read("Documents/header.txt"), header);
    assert_eq!(home.read("Documents/nonce.txt"), nonce);
    let message = home.open("Documents/nonce.txt").unwrap_err().to_string();
    assert!(
        message.contains("Documents/nonce.txt is damaged: its contents failed the integrity check")
    );

    // The rollback copy goes only after the active object and the copy pass
    // the full check.
    let notices = home.ready().unwrap();
    assert_eq!(notices.len(), 1);
    let binary = home.data_dir().join("elastos");
    let writer = crate::install_transaction::InstallationGuard::acquire(home.data_dir()).unwrap();
    let removed = finalize_declared_principal_root_migrations(
        home.data_dir(),
        &home.declarations(),
        &binary,
        &writer,
    )
    .unwrap();
    assert!(removed >= 1, "removed {removed}");
    assert_eq!(home.open("Documents/fragment.bin").unwrap(), fragment);
}

#[test]
fn damaged_candidates_do_not_spend_the_plaintext_budget() {
    let home = Home::new("person:local:scan-damaged-budget-home");
    for index in 0..5 {
        home.write_late_sign(&format!("Documents/a-late-sign-{index}.bin"), 13 * MIB);
    }
    let plaintext = vec![b'p'; 6 * MIB];
    home.write("Documents/z-plain.bin", &plaintext);

    // Start names the one unencrypted file, not the damaged ones.
    let error = home.ready().unwrap_err();
    let outcome = error
        .downcast_ref::<PrincipalRootMigrationRequiredV1>()
        .expect("start refuses with the migration-required outcome");
    assert_eq!(outcome.plaintext_object_count, 1);
    assert_eq!(outcome.examples, vec!["Documents/z-plain.bin".to_string()]);

    // 65 MiB of damaged candidates, past the old 64 MiB readiness budget,
    // neither hide the plaintext file nor turn into a false P3.
    let receipt = home.upgrade(&home.backup_dir()).unwrap();
    assert_eq!(receipt.object_count, 1);
    assert_eq!(receipt.notices.len(), 5);
    assert_eq!(home.open("Documents/z-plain.bin").unwrap(), plaintext);
    assert_eq!(home.ready().unwrap().len(), 5);
}

#[test]
fn probe_cap_names_unread_candidates_and_start_continues() {
    let home = Home::new("person:local:scan-probe-cap-home");
    for index in 0..8 {
        home.write_late_sign(&format!("Documents/a-late-sign-{index}.bin"), 15 * MIB);
    }
    // A genuine unencrypted file that the cap leaves unread.
    let plaintext = vec![b'p'; 6 * MIB];
    home.write("Documents/z-plain.bin", &plaintext);

    let before = home.probes();
    let notices = home.ready().expect("start continues past the probe cap");
    let probes = probe_delta(before, home.probes());
    assert!(
        probes.bytes_read <= MAX_PRINCIPAL_ROOT_SCAN_PROBE_BYTES + 9 * HEAD_WINDOW_MAX as u64,
        "read {} bytes",
        probes.bytes_read
    );
    assert_eq!(notices.len(), 9, "{notices:?}");
    for (index, notice) in notices.iter().take(5).enumerate() {
        assert!(
            notice.starts_with(&format!(
                "Documents/a-late-sign-{index}.bin does not carry the label"
            )),
            "{notice}"
        );
    }
    for (index, notice) in notices.iter().enumerate().skip(5).take(3) {
        assert!(
            notice.starts_with(&format!(
                "Documents/a-late-sign-{index}.bin could not be checked in this pass"
            )),
            "{notice}"
        );
    }
    assert!(
        notices[8].starts_with("Documents/z-plain.bin could not be checked in this pass"),
        "{}",
        notices[8]
    );
    assert!(
        notices[8].contains(
            "Home will not encrypt, change or delete it until it is checked. Move the other \
             files listed with it out of their folders, or move it out of "
        ),
        "{}",
        notices[8]
    );

    // Recovery does not count it as encrypted, and lists it.
    let inspection = inspect_declarative_principal_root_protection(
        home.data_dir(),
        &home.principal_id,
        &home.localhost_root,
        &home.declarations()[0].inventory,
    )
    .unwrap();
    assert_eq!(inspection.encrypted_object_count, 0);
    assert_eq!(inspection.notices.len(), 9);
    assert_eq!(
        inspection.notices[8].damage,
        PrincipalRootObjectDamage::Unchecked
    );

    // The upgrade selects nothing it could not check and changes nothing.
    let receipt = home.upgrade(&home.backup_dir()).unwrap();
    assert_eq!(receipt.object_count, 0);
    assert_eq!(receipt.notices.len(), 9);
    assert_eq!(home.read("Documents/z-plain.bin"), plaintext);
}

#[cfg(unix)]
#[test]
fn linked_declared_folder_is_refused_by_name() {
    let elsewhere = tempfile::tempdir().unwrap();

    // A declared folder that is a link.
    let home = Home::new("person:local:scan-linked-declaration-home");
    let documents = home.path("Documents");
    std::os::unix::fs::symlink(elsewhere.path(), &documents).unwrap();
    assert_eq!(
        home.ready().unwrap_err().to_string(),
        format!(
            "Documents is a link. Protected Home folders hold only regular files and folders. \
             Move it out of {}, then try again.",
            documents.parent().unwrap().display()
        )
    );

    // A link above an exact declaration.
    let home = Home::new("person:local:scan-linked-ancestor-home");
    let notes = home.path("Documents/notes");
    std::os::unix::fs::symlink(elsewhere.path(), &notes).unwrap();
    let declarations = vec![PrincipalRootUpgradeDeclarationV1 {
        principal_id: home.principal_id.clone(),
        localhost_root: home.localhost_root.clone(),
        inventory: vec![PrincipalRootProtectedObjectDeclarationV1::exact(
            home.uri("Documents/notes/a.txt"),
        )],
    }];
    assert_eq!(
        verify_declared_principal_roots_ready(home.data_dir(), &declarations)
            .unwrap_err()
            .to_string(),
        format!(
            "Documents/notes is a link. Protected Home folders hold only regular files and \
             folders. Move it out of {}, then try again.",
            notes.parent().unwrap().display()
        )
    );
}

#[cfg(unix)]
#[test]
fn names_with_line_breaks_or_bidi_render_on_one_line() {
    // A damaged file whose name would forge a second `Error:` line.
    let home = Home::new("person:local:scan-escaped-notice-home");
    let name = "Documents/evil\nError: forged\u{202e}txt.bin";
    let mut damaged = home.envelope(name, b"damaged body");
    damaged[9] = 0xff;
    home.write(name, &damaged);
    let notices = home.ready().unwrap();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].starts_with(
            "Documents/evil\\u{a}Error: forged\\u{202e}txt.bin does not carry the label"
        ),
        "{}",
        notices[0]
    );
    assert!(!notices[0].contains(['\n', '\u{202e}']), "{}", notices[0]);

    // A link inside a folder whose name carries the same characters: the
    // refusal names both on one line.
    let home = Home::new("person:local:scan-escaped-refusal-home");
    let folder = home.path("Documents/x\u{2066}\nError: y/link");
    std::os::unix::fs::symlink("/", &folder).unwrap();
    let message = home.ready().unwrap_err().to_string();
    assert!(!message.contains(['\n', '\u{2066}']), "{message}");
    assert!(
        message.starts_with("Documents/x\\u{2066}\\u{a}Error: y/link is a link."),
        "{message}"
    );
    assert!(
        message.contains("/Documents/x\\u{2066}\\u{a}Error: y, then try again."),
        "{message}"
    );
}

/// #449: the writer's depth rule refuses exactly what the start walk refuses,
/// and names the path on one line.
#[test]
fn writer_depth_rule_matches_the_start_walk() {
    let home = Home::new("person:local:scan-writer-depth-home");
    let inventory = home.declarations().remove(0).inventory;
    let refusal = |relative: &str, folder: bool| {
        refuse_principal_root_depth_breach(
            &home.localhost_root,
            &inventory,
            &home.uri(relative),
            folder,
        )
    };
    let mut deepest = String::from("Documents");
    for level in 1..=15 {
        deepest.push_str(&format!("/{level}"));
    }

    // 15 folders below Documents and a file in the last: both pass.
    refusal(&deepest, true).unwrap();
    refusal(&format!("{deepest}/a.txt"), false).unwrap();
    home.write_envelope(&format!("{deepest}/a.txt"), b"deep");
    assert!(home.ready().unwrap().is_empty());

    // A 16th folder, or a file that needs one: both refused, as the walk
    // refuses the folder. The name is shown on one line.
    let too_deep = format!("{deepest}/x\nError: y");
    assert_eq!(
        refusal(&too_deep, true).unwrap_err().to_string(),
        format!(
            "{deepest}/x\\u{{a}}Error: y was not saved: it would be more than 16 folders deep in \
             Documents, and Home does not start with folders that deep. Save it fewer folders \
             down."
        )
    );
    assert!(refusal(&format!("{too_deep}/a.txt"), false).is_err());
    std::fs::create_dir(home.path(&too_deep)).unwrap();
    assert!(home
        .ready()
        .unwrap_err()
        .to_string()
        .contains("is more than 16 folders deep."));

    // Outside every declared root nothing is refused.
    let outside = deepest.replacen("Documents", "Elsewhere", 1);
    refusal(&format!("{outside}/16/17"), true).unwrap();
}
