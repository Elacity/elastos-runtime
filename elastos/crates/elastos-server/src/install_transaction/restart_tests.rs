use super::*;

const CANDIDATE_PID: u32 = 42001;
const PREVIOUS_PID: u32 = 42002;

struct RestartFixture {
    root: tempfile::TempDir,
    data: PathBuf,
    binary: PathBuf,
}

impl RestartFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let resolved = fs::canonicalize(root.path()).unwrap();
        let data = resolved.join("data");
        let binary = resolved.join("custom-bin/runtime");
        fs::create_dir(&data).unwrap();
        fs::create_dir(binary.parent().unwrap()).unwrap();
        write_new(&data.join("owner-data"), b"owner data before update", 0o600).unwrap();
        let fixture = Self { root, data, binary };
        let writer = fixture.writer();
        for id in ReleaseFile::ALL {
            let path = &writer.destinations[&id];
            writer.check_parent(path.parent().unwrap(), true).unwrap();
            write_new(path, &old_bytes(id), old_mode(id)).unwrap();
        }
        drop(writer);
        fixture
    }

    fn writer(&self) -> InstallTransaction {
        InstallTransaction::acquire(&self.data, &self.binary).unwrap()
    }

    fn prepared(&self) -> InstallTransaction {
        let writer = self.writer();
        prepare_release(&writer);
        writer.prepare_restart(restart_plan()).unwrap();
        writer
    }

    fn activated(&self) -> InstallTransaction {
        let writer = self.prepared();
        writer.commit_checked(|| Ok(())).unwrap();
        writer
    }

    fn assert_release(&self, writer: &InstallTransaction, previous: bool) {
        for id in ReleaseFile::ALL {
            let expected = if previous {
                old_bytes(id)
            } else {
                new_bytes(id)
            };
            let mode = if previous {
                old_mode(id)
            } else if id == ReleaseFile::RuntimeBinary {
                0o755
            } else {
                old_mode(id)
            };
            assert!(state_matches(
                &file_state(&writer.destinations[&id]).unwrap(),
                Some(&sha256(&expected)),
                Some(mode),
            ));
        }
    }

    fn write_new_owner_data(&self) {
        fs::write(self.data.join("owner-data"), b"owner data after activation").unwrap();
        fs::create_dir(self.data.join("chats")).unwrap();
        write_new(
            &self.data.join("chats/new-chat"),
            b"new chat written by candidate",
            0o600,
        )
        .unwrap();
    }

    fn assert_new_owner_data(&self) {
        assert_eq!(
            fs::read(self.data.join("owner-data")).unwrap(),
            b"owner data after activation",
        );
        assert_eq!(
            fs::read(self.data.join("chats/new-chat")).unwrap(),
            b"new chat written by candidate",
        );
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
        fn collect(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            let bytes = metadata.is_file().then(|| fs::read(path).unwrap());
            out.insert(
                path.strip_prefix(root).unwrap().into(),
                (metadata.mode(), bytes),
            );
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    collect(root, &entry.unwrap().path(), out);
                }
            }
        }
        let mut snapshot = BTreeMap::new();
        collect(self.root.path(), self.root.path(), &mut snapshot);
        snapshot
    }
}

fn old_bytes(id: ReleaseFile) -> Vec<u8> {
    format!("previous release {}", id.name()).into_bytes()
}

fn new_bytes(id: ReleaseFile) -> Vec<u8> {
    format!("candidate release {}", id.name()).into_bytes()
}

fn old_mode(id: ReleaseFile) -> u32 {
    if id == ReleaseFile::RuntimeBinary {
        0o750
    } else if matches!(id, ReleaseFile::ReleaseHead | ReleaseFile::ReleaseManifest) {
        0o600
    } else {
        0o640
    }
}

fn restart_plan() -> RestartPlan {
    RestartPlan {
        request_id: "c".repeat(32),
        controller_sha256: sha256(b"verified controller"),
        launch_plan_sha256: sha256(b"private launch plan"),
        support_sha256: sha256(b"frozen support"),
        previous_version: "0.7.0".into(),
        candidate_version: "0.7.1".into(),
        previous_binary_sha256: sha256(&old_bytes(ReleaseFile::RuntimeBinary)),
    }
}

fn prepare_release(writer: &InstallTransaction) {
    let files = ReleaseFile::ALL.map(|id| (id, new_bytes(id)));
    let borrowed = files.each_ref().map(|(id, bytes)| (*id, bytes.as_slice()));
    writer.prepare(&borrowed).unwrap();
}

fn started(writer: &InstallTransaction, previous: bool) -> RestartRecord {
    let claim = writer.claim_start(previous).unwrap();
    writer
        .record_started(
            &claim.generation,
            if previous {
                PREVIOUS_PID
            } else {
                CANDIDATE_PID
            },
            "Sat Oct 3 11:22:33 2026".into(),
        )
        .unwrap();
    writer.restart_record().unwrap()
}

fn assert_no_transaction_scratch(writer: &InstallTransaction) {
    assert!(!writer.journal_path().exists());
    for parent in writer.parents() {
        for directory in [STAGE, ROLLBACK] {
            assert!(!parent.join(directory).exists());
        }
    }
}

#[test]
fn pre_restart_recovery_consumes_unchanged_v3_staging_and_prepared_state() {
    for phase in [Phase::Staging, Phase::Prepared] {
        let fixture = RestartFixture::new();
        let writer = fixture.writer();
        prepare_release(&writer);
        let mut journal = writer.read_journal().unwrap().unwrap();
        assert_eq!(journal.schema, "elastos.install-transaction/v3");
        if phase == Phase::Staging {
            journal.phase = phase;
            writer.write_journal(&journal).unwrap();
            // An interrupted staging copy can leave both finalized and partial files.
            let id = ReleaseFile::ReleaseManifest;
            fs::remove_file(writer.scratch(id, STAGE)).unwrap();
            fs::remove_file(writer.scratch(id, ROLLBACK)).unwrap();
            write_new(&writer.partial(id, STAGE), b"partial candidate", 0o640).unwrap();
        }
        fixture.write_new_owner_data();
        drop(writer);
        let writer = fixture.writer();
        assert!(writer.restart_record_if_any().unwrap().is_none());
        assert!(writer.recover_before_restart().unwrap());
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
        assert_no_transaction_scratch(&writer);
        assert!(!writer.recover_before_restart().unwrap());
        assert!(writer.restart_record_if_any().unwrap().is_none());
    }
}

#[test]
fn pre_restart_recovery_preserves_v4_and_v3_activation_for_their_own_recovery_owner() {
    for activated in [false, true] {
        let fixture = RestartFixture::new();
        let writer = if activated {
            fixture.activated()
        } else {
            fixture.prepared()
        };
        let before = fixture.snapshot();
        assert!(writer.restart_record_if_any().unwrap().is_some());
        assert!(!writer.recover_before_restart().unwrap());
        assert_eq!(fixture.snapshot(), before);
    }
    for phase in [Phase::Committing, Phase::Committed] {
        let fixture = RestartFixture::new();
        let writer = fixture.writer();
        prepare_release(&writer);
        let mut journal = writer.read_journal().unwrap().unwrap();
        journal.phase = phase;
        writer.write_journal(&journal).unwrap();
        let count = if phase == Phase::Committed {
            journal.entries.len()
        } else {
            2
        };
        for entry in journal.entries.iter().take(count) {
            fs::rename(
                writer.scratch(entry.id, STAGE),
                &writer.destinations[&entry.id],
            )
            .unwrap();
        }
        fixture.write_new_owner_data();
        drop(writer);
        let writer = fixture.writer();
        let before = fixture.snapshot();
        assert!(writer.restart_record_if_any().unwrap().is_none());
        assert!(writer.recover_before_restart().is_err());
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_new_owner_data();
    }
}

#[test]
fn restart_requires_a_complete_previous_release_and_its_exact_binary_binding() {
    for missing_previous_file in [false, true] {
        let fixture = RestartFixture::new();
        let writer = fixture.writer();
        if missing_previous_file {
            fs::remove_file(&writer.destinations[&ReleaseFile::ReleaseHead]).unwrap();
        }
        prepare_release(&writer);
        let mut plan = restart_plan();
        if !missing_previous_file {
            plan.previous_binary_sha256 = sha256(b"another previous Runtime");
        }
        let before = fixture.snapshot();
        assert!(writer.prepare_restart(plan).is_err());
        assert_eq!(fixture.snapshot(), before);
        assert!(writer.read_journal().unwrap().unwrap().restart.is_none());
    }
}

#[test]
fn candidate_success_retains_verified_rollback_until_exact_generation_is_ready() {
    let fixture = RestartFixture::new();
    let writer = fixture.activated();
    fixture.assert_release(&writer, false);
    fixture.write_new_owner_data();
    for id in ReleaseFile::ALL {
        assert!(state_matches(
            &file_state(&writer.scratch(id, ROLLBACK)).unwrap(),
            Some(&sha256(&old_bytes(id))),
            Some(old_mode(id)),
        ));
    }
    let before = fixture.snapshot();
    assert!(writer.finish_restart().is_err());
    assert_eq!(fixture.snapshot(), before);

    let claim = writer.claim_start(false).unwrap();
    assert_eq!(claim.phase, RestartPhase::CandidateStartClaimed);
    assert_eq!(claim.generation.len(), 32);
    let before = fixture.snapshot();
    assert!(writer.claim_start(false).is_err());
    assert!(writer.claim_start(true).is_err());
    assert!(writer
        .record_ready(&claim.generation, CANDIDATE_PID)
        .is_err());
    assert!(writer
        .record_started("wrong-generation", CANDIDATE_PID, "birth".into())
        .is_err());
    assert!(writer
        .record_started(&claim.generation, 0, "birth".into())
        .is_err());
    assert!(writer
        .record_started(&claim.generation, CANDIDATE_PID, String::new())
        .is_err());
    assert_eq!(fixture.snapshot(), before);

    writer
        .record_started(&claim.generation, CANDIDATE_PID, "birth".into())
        .unwrap();
    let before = fixture.snapshot();
    assert!(writer.finish_restart().is_err());
    assert!(writer
        .record_ready("wrong-generation", CANDIDATE_PID)
        .is_err());
    assert!(writer
        .record_ready(&claim.generation, PREVIOUS_PID)
        .is_err());
    assert_eq!(fixture.snapshot(), before);
    writer
        .record_ready(&claim.generation, CANDIDATE_PID)
        .unwrap();
    let before = fixture.snapshot();
    assert!(writer.restore_for_restart().is_err());
    assert!(writer
        .record_ready(&claim.generation, CANDIDATE_PID)
        .is_err());
    assert_eq!(fixture.snapshot(), before);
    writer.finish_restart().unwrap();

    fixture.assert_release(&writer, false);
    fixture.assert_new_owner_data();
    assert_no_transaction_scratch(&writer);
    assert!(writer.restart_record().is_err());
}

#[test]
fn candidate_failure_restores_complete_release_preserves_new_data_and_claims_previous_once() {
    // Pending covers refusal before spawn; claimed covers a loader failure;
    // running covers a crashed or stopped candidate before readiness.
    for candidate_phase in [
        RestartPhase::CandidatePending,
        RestartPhase::CandidateStartClaimed,
        RestartPhase::CandidateRunning,
    ] {
        let fixture = RestartFixture::new();
        let writer = fixture.activated();
        if candidate_phase == RestartPhase::CandidateStartClaimed {
            writer.claim_start(false).unwrap();
        } else if candidate_phase == RestartPhase::CandidateRunning {
            started(&writer, false);
        }
        fixture.write_new_owner_data();
        writer.restore_for_restart().unwrap();
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
        let restored = writer.restart_record().unwrap();
        assert_eq!(restored.phase, RestartPhase::Restored);
        assert!(restored.generation.is_empty());
        assert!(restored.pid.is_none());
        assert!(restored.process_start.is_none());
        let before = fixture.snapshot();
        assert!(writer.claim_start(false).is_err());
        assert!(writer.finish_restart().is_err());
        assert_eq!(fixture.snapshot(), before);
        let previous = writer.claim_start(true).unwrap();
        drop(writer);

        let writer = fixture.writer();
        let before = fixture.snapshot();
        assert!(writer.claim_start(true).is_err());
        assert!(writer.claim_start(false).is_err());
        assert!(writer.restore_for_restart().is_err());
        assert_eq!(fixture.snapshot(), before);
        writer
            .record_started(&previous.generation, PREVIOUS_PID, "previous birth".into())
            .unwrap();
        writer
            .record_ready(&previous.generation, PREVIOUS_PID)
            .unwrap();
        writer.finish_restart().unwrap();
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
        assert_no_transaction_scratch(&writer);
    }
}

#[test]
fn previous_start_failure_is_a_durable_terminal_refusal_without_a_second_claim() {
    for running in [false, true] {
        let fixture = RestartFixture::new();
        let writer = fixture.activated();
        writer.claim_start(false).unwrap();
        fixture.write_new_owner_data();
        writer.restore_for_restart().unwrap();
        let claim = writer.claim_start(true).unwrap();
        if running {
            writer
                .record_started(&claim.generation, PREVIOUS_PID, "previous birth".into())
                .unwrap();
        }
        drop(writer);

        let writer = fixture.writer();
        let before = fixture.snapshot();
        assert!(writer.claim_start(true).is_err());
        assert!(writer.claim_start(false).is_err());
        assert!(writer.restore_for_restart().is_err());
        assert!(writer.recover().is_err());
        assert!(writer.abort().is_err());
        assert!(writer.finish_restart().is_err());
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
    }
}

#[test]
fn a_new_writer_keeps_ambiguous_candidate_start_for_the_restart_controller() {
    for running in [false, true] {
        let fixture = RestartFixture::new();
        let writer = fixture.activated();
        let claim = writer.claim_start(false).unwrap();
        if running {
            writer
                .record_started(&claim.generation, CANDIDATE_PID, "candidate birth".into())
                .unwrap();
        }
        fixture.write_new_owner_data();
        drop(writer);

        let writer = fixture.writer();
        let before = fixture.snapshot();
        assert!(writer.recover().is_err());
        assert!(writer.abort().is_err());
        assert!(writer.claim_start(false).is_err());
        assert!(writer.claim_start(true).is_err());
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_release(&writer, false);
        fixture.assert_new_owner_data();
        assert_eq!(
            writer.restart_record().unwrap().generation,
            claim.generation
        );
    }
}

#[test]
fn every_interrupted_activation_prefix_restores_before_one_previous_start() {
    for count in 0..=ReleaseFile::ALL.len() {
        let fixture = RestartFixture::new();
        let writer = fixture.prepared();
        let mut journal = writer.read_journal().unwrap().unwrap();
        journal.phase = Phase::Committing;
        writer.write_journal(&journal).unwrap();
        for entry in journal.entries.iter().take(count) {
            fs::rename(
                writer.scratch(entry.id, STAGE),
                &writer.destinations[&entry.id],
            )
            .unwrap();
        }
        fixture.write_new_owner_data();
        drop(writer);

        let writer = fixture.writer();
        let before = fixture.snapshot();
        assert!(writer.recover().is_err());
        assert!(writer.claim_start(false).is_err());
        assert!(writer.claim_start(true).is_err());
        assert_eq!(fixture.snapshot(), before);
        writer.restore_for_restart().unwrap();
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
        let previous = started(&writer, true);
        writer
            .record_ready(&previous.generation, PREVIOUS_PID)
            .unwrap();
        writer.finish_restart().unwrap();
        assert_no_transaction_scratch(&writer);
    }
}

#[test]
fn activation_failure_restores_and_retains_the_previous_start_claim() {
    for failure in 0..=ReleaseFile::ALL.len() {
        let fixture = RestartFixture::new();
        let writer = fixture.prepared();
        fixture.write_new_owner_data();
        let result = writer.commit_with(
            |id| {
                if ReleaseFile::ALL.get(failure) == Some(&id) {
                    anyhow::bail!("injected activation refusal");
                }
                Ok(())
            },
            || {
                if failure == ReleaseFile::ALL.len() {
                    anyhow::bail!("injected post-activation refusal");
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
        assert_eq!(
            writer.restart_record().unwrap().phase,
            RestartPhase::Restored
        );
        let before = fixture.snapshot();
        assert!(writer.claim_start(false).is_err());
        assert!(writer.finish_restart().is_err());
        assert_eq!(fixture.snapshot(), before);
        let previous = started(&writer, true);
        writer
            .record_ready(&previous.generation, PREVIOUS_PID)
            .unwrap();
        writer.finish_restart().unwrap();
        assert_no_transaction_scratch(&writer);
    }
}

#[test]
fn every_interrupted_restoration_prefix_resumes_without_losing_new_data() {
    for count in 0..=ReleaseFile::ALL.len() {
        let fixture = RestartFixture::new();
        let writer = fixture.activated();
        started(&writer, false);
        let mut journal = writer.read_journal().unwrap().unwrap();
        journal.phase = Phase::Recovering;
        let restart = journal.restart.as_mut().unwrap();
        restart.phase = RestartPhase::Restoring;
        restart.generation.clear();
        restart.pid = None;
        restart.process_start = None;
        writer.write_journal(&journal).unwrap();
        for entry in journal.entries.iter().take(count) {
            fs::rename(
                writer.scratch(entry.id, ROLLBACK),
                &writer.destinations[&entry.id],
            )
            .unwrap();
        }
        fixture.write_new_owner_data();
        drop(writer);

        let writer = fixture.writer();
        let before = fixture.snapshot();
        assert!(writer.recover().is_err());
        assert!(writer.claim_start(true).is_err());
        assert_eq!(fixture.snapshot(), before);
        writer.restore_for_restart().unwrap();
        fixture.assert_release(&writer, true);
        fixture.assert_new_owner_data();
        assert_eq!(
            writer.restart_record().unwrap().phase,
            RestartPhase::Restored
        );
        assert!(writer.claim_start(true).is_ok());
        assert!(writer.claim_start(true).is_err());
    }
}

#[test]
fn restart_journal_refuses_malformed_identity_and_phase_before_any_file_change() {
    use serde_json::{json, Value};
    type Mutation = (&'static str, fn(&mut Value));
    let mutations: &[Mutation] = &[
        ("v1 with restart", |v| {
            v["schema"] = json!("elastos.install-transaction/v1")
        }),
        ("restart schema without restart", |v| {
            v["restart"] = Value::Null
        }),
        ("transaction length", |v| v["transaction_id"] = json!("a")),
        ("transaction alphabet", |v| {
            v["transaction_id"] = json!("g".repeat(32))
        }),
        ("foreign data root", |v| {
            v["data_dir"] = json!("/foreign/data")
        }),
        ("foreign binary basename", |v| {
            v["binary_basename"] = json!("foreign")
        }),
        ("missing release file", |v| {
            v["entries"].as_array_mut().unwrap().pop();
        }),
        ("duplicate release file", |v| {
            v["entries"][4] = v["entries"][0].clone()
        }),
        ("staged hash", |v| {
            v["entries"][1]["staged_sha256"] = json!("bad")
        }),
        ("original hash", |v| {
            v["entries"][1]["original_sha256"] = json!("g".repeat(64))
        }),
        ("missing previous file", |v| {
            v["entries"][1]["original_sha256"] = Value::Null;
            v["entries"][1]["original_mode"] = Value::Null;
        }),
        ("missing previous mode", |v| {
            v["entries"][1]["original_mode"] = Value::Null
        }),
        ("unsafe previous mode", |v| {
            v["entries"][1]["original_mode"] = json!(0o666)
        }),
        ("unsafe candidate mode", |v| {
            v["entries"][1]["staged_mode"] = json!(0o666)
        }),
        ("special candidate mode", |v| {
            v["entries"][0]["staged_mode"] = json!(0o4755)
        }),
        ("controller hash", |v| {
            v["restart"]["plan"]["controller_sha256"] = json!("bad")
        }),
        ("request identity length", |v| {
            v["restart"]["plan"]["request_id"] = json!("c")
        }),
        ("request identity alphabet", |v| {
            v["restart"]["plan"]["request_id"] = json!("g".repeat(32))
        }),
        ("launch plan hash", |v| {
            v["restart"]["plan"]["launch_plan_sha256"] = json!("bad")
        }),
        ("support hash", |v| {
            v["restart"]["plan"]["support_sha256"] = json!("bad")
        }),
        ("previous binary binding", |v| {
            v["restart"]["plan"]["previous_binary_sha256"] = json!("a".repeat(64))
        }),
        ("previous version", |v| {
            v["restart"]["plan"]["previous_version"] = json!("latest")
        }),
        ("candidate version", |v| {
            v["restart"]["plan"]["candidate_version"] = json!("latest")
        }),
        ("unknown restart field", |v| {
            v["restart"]["extra"] = json!(true)
        }),
        ("unknown restart phase", |v| {
            v["restart"]["phase"] = json!("restart_again")
        }),
        ("candidate outer phase", |v| v["phase"] = json!("prepared")),
        ("previous outer phase", |v| {
            v["restart"]["phase"] = json!("previous_running")
        }),
        ("pending claimed identity", |v| {
            v["restart"]["phase"] = json!("candidate_pending")
        }),
        ("claimed running identity", |v| {
            v["restart"]["phase"] = json!("candidate_start_claimed")
        }),
        ("restoring claimed identity", |v| {
            v["phase"] = json!("recovering");
            v["restart"]["phase"] = json!("restoring");
        }),
        ("restored claimed identity", |v| {
            v["phase"] = json!("recovering");
            v["restart"]["phase"] = json!("restored");
        }),
        ("generation length", |v| {
            v["restart"]["generation"] = json!("a")
        }),
        ("generation alphabet", |v| {
            v["restart"]["generation"] = json!("g".repeat(32))
        }),
        ("missing running pid", |v| v["restart"]["pid"] = Value::Null),
        ("zero running pid", |v| v["restart"]["pid"] = json!(0)),
        ("missing running birth", |v| {
            v["restart"]["process_start"] = Value::Null
        }),
        ("empty running birth", |v| {
            v["restart"]["process_start"] = json!("")
        }),
        ("oversized running birth", |v| {
            v["restart"]["process_start"] = json!("a".repeat(129))
        }),
    ];
    for &(name, mutate) in mutations {
        let fixture = RestartFixture::new();
        let writer = fixture.activated();
        let running = started(&writer, false);
        fixture.write_new_owner_data();
        let mut journal: Value =
            serde_json::from_slice(&fs::read(writer.journal_path()).unwrap()).unwrap();
        mutate(&mut journal);
        fs::write(writer.journal_path(), serde_json::to_vec(&journal).unwrap()).unwrap();
        let before = fixture.snapshot();
        assert!(writer.read_journal().is_err(), "{name}");
        assert!(writer.restart_record().is_err(), "{name}");
        assert!(writer.recover().is_err(), "{name}");
        assert!(writer.restore_for_restart().is_err(), "{name}");
        assert!(writer.claim_start(false).is_err(), "{name}");
        assert!(writer.claim_start(true).is_err(), "{name}");
        assert!(writer.finish_restart().is_err(), "{name}");
        assert!(
            authorize_host_start_with_generation(
                &fixture.data,
                &fixture.binary,
                Some(&running.generation),
                CANDIDATE_PID,
            )
            .is_err(),
            "host start: {name}"
        );
        assert_eq!(fixture.snapshot(), before, "{name}");
        fixture.assert_new_owner_data();
    }
}

#[test]
fn foreign_live_or_scratch_changes_preserve_the_whole_set_on_restore_and_cleanup_refusal() {
    for ready in [false, true] {
        for tamper in [
            "live bytes",
            "live mode",
            "rollback bytes",
            "rollback mode",
            "foreign scratch",
        ] {
            let fixture = RestartFixture::new();
            let writer = fixture.activated();
            let running = started(&writer, false);
            if ready {
                writer
                    .record_ready(&running.generation, CANDIDATE_PID)
                    .unwrap();
            }
            fixture.write_new_owner_data();
            match tamper {
                "live bytes" => fs::write(
                    &writer.destinations[&ReleaseFile::Sources],
                    b"owner changed release file",
                )
                .unwrap(),
                "live mode" => fs::set_permissions(
                    &writer.destinations[&ReleaseFile::Sources],
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap(),
                "rollback bytes" => fs::write(
                    writer.scratch(ReleaseFile::Components, ROLLBACK),
                    b"foreign rollback",
                )
                .unwrap(),
                "rollback mode" => fs::set_permissions(
                    writer.scratch(ReleaseFile::Components, ROLLBACK),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap(),
                "foreign scratch" => write_new(
                    &writer
                        .scratch(ReleaseFile::RuntimeBinary, ROLLBACK)
                        .parent()
                        .unwrap()
                        .join("foreign"),
                    b"foreign scratch",
                    0o600,
                )
                .unwrap(),
                _ => unreachable!(),
            }
            let before = fixture.snapshot();
            assert!(
                writer.restore_for_restart().is_err(),
                "{tamper}, ready={ready}"
            );
            assert!(writer.finish_restart().is_err(), "{tamper}, ready={ready}");
            assert_eq!(fixture.snapshot(), before, "{tamper}, ready={ready}");
            fixture.assert_new_owner_data();
        }
    }
}

#[test]
fn cli_transaction_refusals_name_cli_recovery_and_preserve_release_files() {
    let fixture = RestartFixture::new();
    let writer = fixture.writer();
    prepare_release(&writer);
    let before = fixture.snapshot();
    for result in [
        authorize_host_start_with_generation(&fixture.data, &fixture.binary, None, CANDIDATE_PID),
        refuse_pending_home_start(&fixture.data, &fixture.binary),
    ] {
        let message = result.unwrap_err().to_string();
        assert!(message.contains("Run `elastos update` again"), "{message}");
        assert!(!message.contains("controller"), "{message}");
        assert_eq!(fixture.snapshot(), before);
    }
    writer.recover().unwrap();
    fixture.assert_release(&writer, true);
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        None,
        CANDIDATE_PID
    )
    .is_ok());
    assert!(refuse_pending_home_start(&fixture.data, &fixture.binary).is_ok());
}

#[test]
fn controller_transaction_refusals_keep_the_controller_recovery_owner() {
    let fixture = RestartFixture::new();
    let writer = fixture.activated();
    let before = fixture.snapshot();
    for result in [
        authorize_host_start_with_generation(&fixture.data, &fixture.binary, None, CANDIDATE_PID),
        refuse_pending_home_start(&fixture.data, &fixture.binary),
    ] {
        let message = result.unwrap_err().to_string();
        assert!(message.contains("retained update controller"), "{message}");
        assert!(!message.contains("`elastos update`"), "{message}");
        assert_eq!(fixture.snapshot(), before);
    }
    assert!(writer.recover().is_err());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn host_start_fence_admits_only_the_claimed_generation_and_binary() {
    let fixture = RestartFixture::new();
    let writer = fixture.writer();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        None,
        CANDIDATE_PID
    )
    .is_ok());
    drop(writer);
    let writer = fixture.activated();
    let before = fixture.snapshot();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        None,
        CANDIDATE_PID
    )
    .is_err());
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&"a".repeat(32)),
        CANDIDATE_PID
    )
    .is_err());
    assert_eq!(fixture.snapshot(), before);
    let claim = writer.claim_start(false).unwrap();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&claim.generation),
        CANDIDATE_PID
    )
    .is_ok());
    writer
        .record_started(&claim.generation, CANDIDATE_PID, "candidate birth".into())
        .unwrap();
    let before = fixture.snapshot();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&claim.generation),
        PREVIOUS_PID
    )
    .is_err());
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&"a".repeat(32)),
        CANDIDATE_PID
    )
    .is_err());
    assert_eq!(fixture.snapshot(), before);

    fs::write(&fixture.binary, b"foreign runtime").unwrap();
    let before = fixture.snapshot();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&claim.generation),
        CANDIDATE_PID
    )
    .is_err());
    assert_eq!(fixture.snapshot(), before);
    fs::write(&fixture.binary, new_bytes(ReleaseFile::RuntimeBinary)).unwrap();
    writer.restore_for_restart().unwrap();
    let previous = writer.claim_start(true).unwrap();
    assert_ne!(previous.generation, claim.generation);
    let before = fixture.snapshot();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&claim.generation),
        PREVIOUS_PID
    )
    .is_err());
    assert_eq!(fixture.snapshot(), before);
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&previous.generation),
        PREVIOUS_PID
    )
    .is_ok());
    writer
        .record_started(&previous.generation, PREVIOUS_PID, "previous birth".into())
        .unwrap();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&previous.generation),
        PREVIOUS_PID
    )
    .is_ok());
    writer
        .record_ready(&previous.generation, PREVIOUS_PID)
        .unwrap();
    let before = fixture.snapshot();
    assert!(authorize_host_start_with_generation(
        &fixture.data,
        &fixture.binary,
        Some(&previous.generation),
        PREVIOUS_PID
    )
    .is_err());
    assert_eq!(fixture.snapshot(), before);
}

fn publisher_sentinel(fixture: &RestartFixture) -> Vec<(PathBuf, Vec<u8>)> {
    let mut sentinels = Vec::new();
    for path in [
        publisher_release_head_path(&fixture.data),
        publisher_release_manifest_path(&fixture.data),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = format!(
            "Publisher owns {}",
            path.file_name().unwrap().to_str().unwrap()
        )
        .into_bytes();
        write_new(&path, &bytes, 0o600).unwrap();
        sentinels.push((path, bytes));
    }
    sentinels
}

fn assert_sentinels(sentinels: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in sentinels {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
}

fn move_journal_to_legacy(
    fixture: &RestartFixture,
    writer: InstallTransaction,
    schema: &str,
) -> (InstallTransaction, Vec<(PathBuf, Vec<u8>)>) {
    let mut journal = writer.read_journal().unwrap().unwrap();
    journal.schema = schema.into();
    writer.write_journal(&journal).unwrap();
    drop(writer);
    let publisher = publisher_release_head_path(&fixture.data)
        .parent()
        .unwrap()
        .to_path_buf();
    fs::create_dir_all(publisher.parent().unwrap()).unwrap();
    fs::rename(fixture.data.join("installation"), &publisher).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(fixture.data.join("installation"))
        .unwrap();
    let mut sentinels = Vec::new();
    for path in [
        installation_release_head_path(&fixture.data),
        installation_release_manifest_path(&fixture.data),
    ] {
        let bytes = format!(
            "Consumed owner {}",
            path.file_name().unwrap().to_str().unwrap()
        )
        .into_bytes();
        write_new(&path, &bytes, 0o600).unwrap();
        sentinels.push((path, bytes));
    }
    let writer = fixture.writer();
    assert!(!writer.uses_consumed_layout());
    assert_eq!(
        writer.release_head_path(),
        publisher_release_head_path(&fixture.data)
    );
    assert_eq!(
        writer.release_manifest_path(),
        publisher_release_manifest_path(&fixture.data)
    );
    (writer, sentinels)
}

#[test]
fn legacy_v1_recovery_keeps_its_publisher_map_and_object_recovery_only() {
    for phase in [Phase::Staging, Phase::Prepared] {
        let fixture = RestartFixture::new();
        let writer = fixture.writer();
        prepare_release(&writer);
        let mut journal = writer.read_journal().unwrap().unwrap();
        journal.phase = phase;
        writer.write_journal(&journal).unwrap();
        let (writer, sentinels) =
            move_journal_to_legacy(&fixture, writer, "elastos.install-transaction/v1");
        fixture.write_new_owner_data();
        assert!(writer.prepare_restart(restart_plan()).is_err());
        assert!(writer.recover_before_restart().unwrap());
        fixture.assert_release(&writer, true);
        assert_no_transaction_scratch(&writer);
        let before = fixture.snapshot();
        let files = ReleaseFile::ALL.map(|id| (id, new_bytes(id)));
        let borrowed = files.each_ref().map(|(id, bytes)| (*id, bytes.as_slice()));
        assert!(writer.prepare(&borrowed).is_err());
        assert!(writer.prepare_restart(restart_plan()).is_err());
        assert_eq!(fixture.snapshot(), before);
        assert_sentinels(&sentinels);
        fixture.assert_new_owner_data();
        drop(writer);
        assert!(fixture.writer().uses_consumed_layout());
    }
}

#[test]
fn legacy_v2_restore_previous_start_and_cleanup_touch_only_publisher_slots() {
    let fixture = RestartFixture::new();
    let activated = fixture.activated();
    let (writer, sentinels) =
        move_journal_to_legacy(&fixture, activated, "elastos.install-transaction/v2");
    fixture.write_new_owner_data();
    assert!(writer.prepare_restart(restart_plan()).is_err());
    writer.restore_for_restart().unwrap();
    fixture.assert_release(&writer, true);
    let running = started(&writer, true);
    writer
        .record_ready(&running.generation, PREVIOUS_PID)
        .unwrap();
    writer.finish_restart().unwrap();
    assert_no_transaction_scratch(&writer);
    assert!(writer.prepare_restart(restart_plan()).is_err());
    let files = ReleaseFile::ALL.map(|id| (id, new_bytes(id)));
    let borrowed = files.each_ref().map(|(id, bytes)| (*id, bytes.as_slice()));
    assert!(writer.prepare(&borrowed).is_err());
    assert_sentinels(&sentinels);
    fixture.assert_new_owner_data();
}

#[test]
fn consumed_v3_and_v4_activation_recovery_preserve_publisher_publication() {
    for restart in [false, true] {
        let fixture = RestartFixture::new();
        let sentinels = publisher_sentinel(&fixture);
        let writer = if restart {
            fixture.activated()
        } else {
            let writer = fixture.writer();
            prepare_release(&writer);
            writer.activate_artifacts_for_support().unwrap();
            writer
        };
        assert!(writer.uses_consumed_layout());
        assert_eq!(
            writer.read_journal().unwrap().unwrap().schema,
            if restart {
                "elastos.install-transaction/v4"
            } else {
                "elastos.install-transaction/v3"
            }
        );
        fixture.write_new_owner_data();
        if restart {
            writer.restore_for_restart().unwrap();
            let running = started(&writer, true);
            writer
                .record_ready(&running.generation, PREVIOUS_PID)
                .unwrap();
            writer.finish_restart().unwrap();
        } else {
            writer.recover().unwrap();
        }
        fixture.assert_release(&writer, true);
        assert_no_transaction_scratch(&writer);
        assert_sentinels(&sentinels);
        fixture.assert_new_owner_data();
    }
}

#[test]
fn normal_interrupted_binary_components_and_support_seams_keep_all_five_old_backups() {
    for seam in ["binary", "components", "support"] {
        let fixture = RestartFixture::new();
        let sentinels = publisher_sentinel(&fixture);
        let writer = fixture.writer();
        prepare_release(&writer);
        if seam == "binary" {
            let mut journal = writer.read_journal().unwrap().unwrap();
            journal.phase = Phase::Committing;
            writer.write_journal(&journal).unwrap();
            fs::rename(
                writer.scratch(ReleaseFile::RuntimeBinary, STAGE),
                writer.binary_path(),
            )
            .unwrap();
        } else {
            writer.activate_artifacts_for_support().unwrap();
            if seam == "support" {
                write_new(
                    &fixture.data.join("support-owner-file"),
                    b"separately owned support remains",
                    0o600,
                )
                .unwrap();
            }
        }
        for entry in &writer.read_journal().unwrap().unwrap().entries {
            assert_eq!(
                fs::read(writer.scratch(entry.id, ROLLBACK)).unwrap(),
                old_bytes(entry.id),
                "{seam}"
            );
        }
        assert!(InstallationGuard::acquire(writer.binary_path().parent().unwrap()).is_err());
        drop(writer);
        let writer = fixture.writer();
        writer.recover().unwrap();
        fixture.assert_release(&writer, true);
        assert_no_transaction_scratch(&writer);
        assert_sentinels(&sentinels);
        if seam == "support" {
            assert_eq!(
                fs::read(fixture.data.join("support-owner-file")).unwrap(),
                b"separately owned support remains"
            );
        }
    }
}

#[test]
fn normal_admitted_prefix_finishes_all_five_consumed_roles() {
    let fixture = RestartFixture::new();
    let sentinels = publisher_sentinel(&fixture);
    let writer = fixture.writer();
    prepare_release(&writer);
    writer.activate_artifacts_for_support().unwrap();
    writer.commit_checked(|| Ok(())).unwrap();
    fixture.assert_release(&writer, false);
    assert_no_transaction_scratch(&writer);
    assert_sentinels(&sentinels);
}
