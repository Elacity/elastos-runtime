use std::fs::{self, OpenOptions};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signer as _, SigningKey};
use elastos_auth::ethereum_signed_message_hash;
use elastos_protected_content_contracts::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationStatementV1,
    CustodyEpochIssuerKeyV1, CustodyEpochStatementV1, CustodyNodeIdentityV1,
    CustodyNodeProvisioningRecordV1, CustodyPoolFailureDomainIdV1, CustodyPoolMemberStateV1,
    CustodyPoolMemberV1, CustodyPoolOperatorIdV1, CustodyPoolStatementV1, Digest32,
    EncryptedContentIdentityV1, EvmContractAddressV1, EvmFunctionSelectorV1, EvmRightsMethodAbiV1,
    KeyReleaseRequestV1, NodePublicKey, ProtectedContentBindingV1,
    RecipientKeyAuthorizationStatementV1, RecipientKeyIdentityV1, RecipientPublicKeyBytesV1,
    ReplayNonce16, RightsActionV1, RightsDecisionV1, RightsEvaluationEvidenceRequestV1,
    RightsObservationFinalityV1, RightsPolicyBodyV1, RightsRequestV1, RightsSubjectSourceV1,
    RuntimeCustodyProvisioningIdV1, RuntimeCustodyProvisioningStatementV1,
    RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1, RuntimeReleaseOperationStatementV1,
    RuntimeSessionBindingV1, SignedCustodyCommitteeAuthorizationV1, SignedCustodyEpochV1,
    SignedCustodyPoolV1, SignedNodeRightsDecisionV1, SignedRecipientKeyAuthorizationV1,
    SignedRuntimeCustodyProvisioningV1, SignedRuntimeReleaseOperationV1, ThresholdV1,
    ValidatedCustodyCommitteeV1, WalletAddress, WalletSignedRightsRequestV1,
    CUSTODY_HPKE_SUITE_ID_V1,
};
use elastos_protected_content_custody::{
    provision_custody_envelope, ContentEncryptionKeyV1, NodeCustodySecretKeyV1,
};
use elastos_protected_content_provider_contracts::{
    CustodyProviderRequestV1, CustodyProviderResponseStatusV1, CustodyProviderResponseV1,
};
use k256::ecdsa::SigningKey as WalletSigningKey;
use sha2::Digest as _;
use sha3::Keccak256;
use zeroize::Zeroizing;

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn issued_unix_seconds() -> u64 {
    now_unix_seconds().saturating_sub(5)
}

fn expires_unix_seconds() -> u64 {
    now_unix_seconds() + 40
}

struct ProviderProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ProviderProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_custody-provider"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn request(&mut self, value: serde_json::Value) -> serde_json::Value {
        serde_json::to_writer(&mut self.stdin, &value).unwrap();
        writeln!(self.stdin).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn stop(mut self) {
        let _ = self.request(serde_json::json!({"op": "shutdown"}));
        drop(self.stdin);
        let _ = self.child.wait();
    }

    fn shutdown_and_wait_with_stdin_open(mut self) {
        let response = self.request(serde_json::json!({"op": "shutdown"}));
        assert_eq!(response["status"], "ok");
        for _ in 0..50 {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        panic!("custody provider did not exit after shutdown acknowledgement");
    }
}

fn digest(seed: u8) -> Digest32 {
    Digest32::new([seed; 32])
}

fn node_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
}

fn node_custody_secret(seed: u8) -> NodeCustodySecretKeyV1 {
    NodeCustodySecretKeyV1::from_guarded_bytes(Zeroizing::new([seed; 32])).unwrap()
}

fn recipient_public_key(seed: u8) -> RecipientPublicKeyBytesV1 {
    let public = node_custody_secret(seed).public_key().unwrap();
    RecipientPublicKeyBytesV1::new(*public.as_bytes()).unwrap()
}

fn recipient_identity(seed: u8) -> RecipientKeyIdentityV1 {
    recipient_public_key(seed)
        .key_identity(CUSTODY_HPKE_SUITE_ID_V1)
        .unwrap()
}

fn custody_policy_key() -> SigningKey {
    SigningKey::from_bytes(&[0x71; 32])
}

fn custody_policy_issuer() -> CustodyEpochIssuerKeyV1 {
    CustodyEpochIssuerKeyV1::new(custody_policy_key().verifying_key().to_bytes()).unwrap()
}

fn custody_member(node_seed: u8) -> CustodyPoolMemberV1 {
    CustodyPoolMemberV1::new(
        node_public_key(node_seed),
        node_custody_secret(node_seed).public_key().unwrap(),
        CustodyPoolOperatorIdV1::new([0x80 + node_seed; 32]),
        CustodyPoolFailureDomainIdV1::new([0x90 + node_seed; 32]),
        CustodyApprovedSuitesV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
        )
        .unwrap(),
        (issued_unix_seconds(), now_unix_seconds() + 600),
        CustodyPoolMemberStateV1::Active,
    )
    .unwrap()
}

fn signed_pool() -> SignedCustodyPoolV1 {
    let key = custody_policy_key();
    let statement = CustodyPoolStatementV1::new(
        custody_policy_issuer(),
        vec![custody_member(1), custody_member(2), custody_member(3)],
    )
    .unwrap();
    SignedCustodyPoolV1::new(
        statement.clone(),
        key.sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn signed_epoch() -> SignedCustodyEpochV1 {
    let key = custody_policy_key();
    let nodes = (1..=3)
        .map(|seed| {
            CustodyNodeIdentityV1::new(
                node_public_key(seed),
                node_custody_secret(seed).public_key().unwrap(),
                elastos_protected_content_contracts::ShareCoordinateV1::new(seed).unwrap(),
            )
            .unwrap()
        })
        .collect();
    let statement = CustodyEpochStatementV1::new(
        custody_policy_issuer(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
            CUSTODY_HPKE_SUITE_ID_V1,
        )
        .unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        nodes,
    )
    .unwrap();
    SignedCustodyEpochV1::new(
        statement.clone(),
        key.sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn validated_committee() -> ValidatedCustodyCommitteeV1 {
    let pool = signed_pool();
    let epoch = signed_epoch();
    let key = custody_policy_key();
    let statement = CustodyCommitteeAuthorizationStatementV1::new(
        custody_policy_issuer(),
        pool.pool_identity().unwrap(),
        epoch.epoch_identity().unwrap(),
    )
    .unwrap();
    let authorization = SignedCustodyCommitteeAuthorizationV1::new(
        statement.clone(),
        key.sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    elastos_protected_content_contracts::validate_custody_epoch_against_pool_at(
        custody_policy_issuer(),
        authorization.authorization_identity().unwrap(),
        &pool,
        &epoch,
        &authorization,
        now_unix_seconds(),
    )
    .unwrap()
}

fn wallet(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

fn policy_body() -> RightsPolicyBodyV1 {
    RightsPolicyBodyV1::new(
        "content:alpha",
        RightsActionV1::View,
        "view",
        RightsSubjectSourceV1::WalletAddress,
        11155111,
        EvmContractAddressV1::new([0x11; 20]).unwrap(),
        EvmFunctionSelectorV1::new([0x12, 0x34, 0x56, 0x78]).unwrap(),
        EvmRightsMethodAbiV1::HasAccessByContentIdStringAddressString,
        RightsObservationFinalityV1::new(12),
    )
    .unwrap()
}

fn signed_rights_request(
    binding: ProtectedContentBindingV1,
    issued_at: u64,
    expires_at: u64,
) -> WalletSignedRightsRequestV1 {
    let request = RightsRequestV1::new(
        binding,
        RightsActionV1::View,
        recipient_identity(0x30),
        issued_at,
        expires_at,
        ReplayNonce16::new([0x55; 16]),
    )
    .unwrap();
    let key = WalletSigningKey::from_slice(&[7; 32]).unwrap();
    let (signature, recovery_id) = key
        .sign_prehash_recoverable(&ethereum_signed_message_hash(
            &request.canonical_bytes().unwrap(),
        ))
        .unwrap();
    let mut signature_bytes = signature.to_bytes().to_vec();
    signature_bytes.push(recovery_id.to_byte());
    WalletSignedRightsRequestV1::new(request, signature_bytes).unwrap()
}

fn signed_release_operation(
    record: &CustodyNodeProvisioningRecordV1,
) -> SignedRuntimeReleaseOperationV1 {
    signed_release_operation_with_runtime_seed(record, 0x42)
}

fn signed_release_operation_with_runtime_seed(
    record: &CustodyNodeProvisioningRecordV1,
    runtime_seed: u8,
) -> SignedRuntimeReleaseOperationV1 {
    let runtime_key = node_signing_key(runtime_seed);
    let policy = policy_body();
    let issued_at = issued_unix_seconds();
    let expires_at = issued_at + 45;
    let binding = ProtectedContentBindingV1::new(
        record.manifest().encrypted_content().clone(),
        record.key_envelope_identity().clone(),
        policy.policy_identity().unwrap(),
        elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
            node_signing_key(0x26).verifying_key().to_bytes(),
        )
        .unwrap(),
        wallet(7),
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap();
    let rights = signed_rights_request(binding.clone(), issued_at, expires_at);
    let release = KeyReleaseRequestV1::new(
        binding.clone(),
        rights.request().request_hash().unwrap(),
        RightsActionV1::View,
        rights.request().recipient().clone(),
        issued_at,
        expires_at,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    let recipient_key = recipient_public_key(0x30);
    let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
        binding.clone(),
        RightsActionV1::View,
        recipient_key,
        rights.request().recipient().clone(),
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        issued_at,
        expires_at,
    )
    .unwrap();
    let profile_key = node_signing_key(0x26);
    let authorization = SignedRecipientKeyAuthorizationV1::new(
        authorization_statement.clone(),
        profile_key
            .sign(&authorization_statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let evidence_request =
        RightsEvaluationEvidenceRequestV1::new(binding, policy.policy_identity().unwrap()).unwrap();
    let statement = RuntimeReleaseOperationStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        rights,
        release,
        recipient_key,
        authorization,
        policy,
        evidence_request,
        signed_epoch(),
        RuntimeReleaseAuditIdV1::new(digest(0x91)).unwrap(),
        issued_at,
        expires_at,
    )
    .unwrap();
    SignedRuntimeReleaseOperationV1::new(
        statement.clone(),
        runtime_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn signed_decision(operation: &SignedRuntimeReleaseOperationV1) -> SignedNodeRightsDecisionV1 {
    signed_decision_with(operation, 1, RightsDecisionV1::Allowed)
}

fn signed_decision_with(
    operation: &SignedRuntimeReleaseOperationV1,
    node_seed: u8,
    decision: RightsDecisionV1,
) -> SignedNodeRightsDecisionV1 {
    let authenticated = operation
        .verify(
            operation.statement().runtime_operation_issuer(),
            now_unix_seconds(),
        )
        .unwrap();
    let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.rights_request_hash(),
        authenticated.binding().clone(),
        authenticated.action(),
        node_public_key(node_seed),
        decision,
        digest(0x80),
        authenticated.statement().release_request().issued_at(),
        authenticated.statement().release_request().expires_at(),
    )
    .unwrap();
    SignedNodeRightsDecisionV1::new(
        statement.clone(),
        node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn signed_provisioning(
    record: &CustodyNodeProvisioningRecordV1,
) -> SignedRuntimeCustodyProvisioningV1 {
    signed_provisioning_with_runtime_seed(record, 0x42)
}

fn signed_provisioning_with_runtime_seed(
    record: &CustodyNodeProvisioningRecordV1,
    runtime_seed: u8,
) -> SignedRuntimeCustodyProvisioningV1 {
    let runtime_key = node_signing_key(runtime_seed);
    let statement = RuntimeCustodyProvisioningStatementV1::new(
        RuntimeOperationIssuerKeyV1::new(runtime_key.verifying_key().to_bytes()).unwrap(),
        record.record_identity().unwrap(),
        RuntimeCustodyProvisioningIdV1::new(digest(0xa5)).unwrap(),
        issued_unix_seconds(),
        expires_unix_seconds(),
    )
    .unwrap();
    SignedRuntimeCustodyProvisioningV1::new(
        statement.clone(),
        runtime_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn provisioning_record() -> CustodyNodeProvisioningRecordV1 {
    let envelope = provision_custody_envelope(
        EncryptedContentIdentityV1::new(digest(0x11), 4096).unwrap(),
        &ContentEncryptionKeyV1::generate().unwrap(),
        &validated_committee(),
    )
    .unwrap();
    CustodyNodeProvisioningRecordV1::new(
        envelope.key_envelope_identity().unwrap(),
        envelope.manifest().clone(),
        node_public_key(1),
        envelope
            .stored_share_for_node(node_public_key(1))
            .unwrap()
            .clone(),
    )
    .unwrap()
}

fn write_owner_only(path: &Path, value: &str) {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    let mut file = options.open(path).unwrap();
    file.write_all(value.as_bytes()).unwrap();
    file.sync_all().unwrap();
}

fn prepare_config(root: &Path) -> serde_json::Value {
    prepare_config_with_seeds(root, 1, 1)
}

fn prepare_config_with_seeds(root: &Path, custody_seed: u8, signing_seed: u8) -> serde_json::Value {
    use std::os::unix::fs::PermissionsExt;

    let root = fs::canonicalize(root).unwrap();
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = root.join("runtime");
    let custody = root.join("custody");
    let signing = root.join("signing");
    write_owner_only(
        &runtime,
        &format!(
            "0x{}",
            hex_bytes(
                RuntimeOperationIssuerKeyV1::new(node_signing_key(0x42).verifying_key().to_bytes())
                    .unwrap()
                    .as_bytes()
            )
        ),
    );
    write_owner_only(&custody, &format!("0x{}", hex_bytes(&[custody_seed; 32])));
    write_owner_only(&signing, &format!("0x{}", hex_bytes(&[signing_seed; 32])));
    serde_json::json!({
        "op": "init",
        "config": {
            "extra": {
                "trusted_runtime_issuer_path": runtime,
                "node_custody_secret_path": custody,
                "node_signing_key_path": signing,
                "data_root_path": data
            }
        }
    })
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn request_value(request: &CustodyProviderRequestV1) -> serde_json::Value {
    serde_json::from_slice(&request.to_json_vec().unwrap()).unwrap()
}

#[test]
fn custody_provider_process_provisions_releases_replays_after_restart_and_shuts_down() {
    let temp = tempfile::tempdir().unwrap();
    let record = provisioning_record();
    let provisioning = signed_provisioning(&record);
    let provision_request =
        CustodyProviderRequestV1::new_provision_node_share(&record, &provisioning).unwrap();
    let release_operation = signed_release_operation(&record);
    let decision = signed_decision(&release_operation);
    let release_request =
        CustodyProviderRequestV1::new_release_contribution(&release_operation, &decision).unwrap();

    let mut provider = ProviderProcess::start();
    let init = provider.request(prepare_config(temp.path()));
    assert_eq!(init["status"], "ok");
    assert_eq!(init["data"]["provider"], "custody");
    assert_eq!(init["data"]["configured"], true);

    let provisioned = provider.request(request_value(&provision_request));
    assert_eq!(provisioned["status"], "ok");
    let provisioned_response: CustodyProviderResponseV1 =
        serde_json::from_value(provisioned["data"].clone()).unwrap();
    assert_eq!(
        provisioned_response.status(),
        CustodyProviderResponseStatusV1::Provisioned
    );
    assert_eq!(
        provisioned_response.provisioned_record_identity().unwrap(),
        record.record_identity().unwrap()
    );

    let duplicate = provider.request(request_value(&provision_request));
    assert_eq!(duplicate["data"], provisioned["data"]);

    let contribution = provider.request(request_value(&release_request));
    assert_eq!(contribution["status"], "ok", "{contribution}");
    let contribution_response: CustodyProviderResponseV1 =
        serde_json::from_value(contribution["data"].clone()).unwrap();
    let signed_contribution = contribution_response.signed_node_contribution().unwrap();
    let authenticated = release_operation
        .verify(
            release_operation.statement().runtime_operation_issuer(),
            now_unix_seconds(),
        )
        .unwrap();
    let node_set = signed_epoch().statement().node_set().unwrap();
    authenticated
        .verify_node_contribution(&signed_contribution, &node_set, now_unix_seconds())
        .unwrap();

    let replay = provider.request(request_value(&release_request));
    assert_eq!(replay["data"], contribution["data"]);
    provider.stop();

    let mut restarted = ProviderProcess::start();
    assert_eq!(
        restarted.request(prepare_config(temp.path()))["status"],
        "ok"
    );
    let restart_replay = restarted.request(request_value(&release_request));
    assert_eq!(restart_replay["data"], contribution["data"]);
    restarted.stop();

    let mut wrong_secret = ProviderProcess::start();
    assert_eq!(
        wrong_secret.request(prepare_config_with_seeds(temp.path(), 2, 1))["status"],
        "ok"
    );
    let wrong_secret_response =
        serde_json::to_string(&wrong_secret.request(request_value(&provision_request))).unwrap();
    assert!(wrong_secret_response.contains("backend_unavailable"));
    assert!(!wrong_secret_response.contains("0x02"));
    wrong_secret.stop();

    let mut wrong_signing = ProviderProcess::start();
    assert_eq!(
        wrong_signing.request(prepare_config_with_seeds(temp.path(), 1, 2))["status"],
        "ok"
    );
    let wrong_signing_response =
        serde_json::to_string(&wrong_signing.request(request_value(&release_request))).unwrap();
    assert!(wrong_signing_response.contains("invalid_request"));
    assert!(!wrong_signing_response.contains("0x02"));
    wrong_signing.stop();
}

#[test]
fn custody_provider_process_shutdown_exits_while_stdin_remains_open() {
    ProviderProcess::start().shutdown_and_wait_with_stdin_open();
}

#[test]
fn custody_provider_process_rejects_wrong_issuer_node_conflict_and_redacts_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let record = provisioning_record();
    let provisioning = signed_provisioning(&record);
    let provision_request =
        CustodyProviderRequestV1::new_provision_node_share(&record, &provisioning).unwrap();
    let mut provider = ProviderProcess::start();
    assert_eq!(
        provider.request(prepare_config(temp.path()))["status"],
        "ok"
    );
    assert_eq!(
        provider.request(request_value(&provision_request))["status"],
        "ok"
    );

    let mut conflicting_record = record.clone();
    let mut share = conflicting_record.sealed_share().clone();
    let mut ciphertext = *share.ciphertext();
    ciphertext[0] ^= 1;
    share = elastos_protected_content_contracts::HpkeCiphertextV1::new(
        *share.encapped_key(),
        ciphertext,
    )
    .unwrap();
    conflicting_record = CustodyNodeProvisioningRecordV1::new(
        record.key_envelope_identity().clone(),
        record.manifest().clone(),
        record.selected_node_public_key(),
        share,
    )
    .unwrap();
    let conflict_request = CustodyProviderRequestV1::new_provision_node_share(
        &conflicting_record,
        &signed_provisioning(&conflicting_record),
    )
    .unwrap();
    let conflict =
        serde_json::to_string(&provider.request(request_value(&conflict_request))).unwrap();
    assert!(conflict.contains("backend_unavailable"));
    assert!(!conflict.contains(temp.path().to_str().unwrap()));
    assert!(!conflict.contains("sealed_share"));
    assert!(!conflict.contains("stored_share"));
    assert!(!conflict.contains("ciphertext"));
    assert!(!conflict.contains("carrier"));
    assert!(!conflict.contains("port"));

    let wrong_issuer_request = CustodyProviderRequestV1::new_provision_node_share(
        &record,
        &signed_provisioning_with_runtime_seed(&record, 0x43),
    )
    .unwrap();
    let wrong_issuer =
        serde_json::to_string(&provider.request(request_value(&wrong_issuer_request))).unwrap();
    assert!(wrong_issuer.contains("invalid_request"));
    assert!(!wrong_issuer.contains("issuer"));
    assert!(!wrong_issuer.contains("0x43"));

    let wrong_runtime_operation = signed_release_operation(&record);
    let wrong_release_issuer_operation = signed_release_operation_with_runtime_seed(&record, 0x43);
    let wrong_release_issuer_decision = signed_decision(&wrong_release_issuer_operation);
    let wrong_release_issuer_request = CustodyProviderRequestV1::new_release_contribution(
        &wrong_release_issuer_operation,
        &wrong_release_issuer_decision,
    )
    .unwrap();
    let wrong_release_issuer =
        serde_json::to_string(&provider.request(request_value(&wrong_release_issuer_request)))
            .unwrap();
    assert!(wrong_release_issuer.contains("invalid_request"));
    assert!(!wrong_release_issuer.contains("issuer"));

    let wrong_node_decision =
        signed_decision_with(&wrong_runtime_operation, 2, RightsDecisionV1::Allowed);
    let wrong_node_request = CustodyProviderRequestV1::new_release_contribution(
        &wrong_runtime_operation,
        &wrong_node_decision,
    )
    .unwrap();
    let wrong_node =
        serde_json::to_string(&provider.request(request_value(&wrong_node_request))).unwrap();
    assert!(wrong_node.contains("invalid_request"));
    assert!(!wrong_node.contains("selected_node"));

    let denied_decision =
        signed_decision_with(&wrong_runtime_operation, 1, RightsDecisionV1::Denied);
    let denied_request = CustodyProviderRequestV1::new_release_contribution(
        &wrong_runtime_operation,
        &denied_decision,
    )
    .unwrap();
    let denied = provider.request(request_value(&denied_request));
    assert_eq!(denied["status"], "error", "{denied}");
    assert_eq!(denied["code"], "rights_denied", "{denied}");
    assert!(denied.get("data").is_none());
    let denied_text = serde_json::to_string(&denied).unwrap();
    assert!(!denied_text.contains("backend_unavailable"));
    assert!(!denied_text.contains("signed_node_contribution"));

    let allowed_after_denial = CustodyProviderRequestV1::new_release_contribution(
        &wrong_runtime_operation,
        &signed_decision(&wrong_runtime_operation),
    )
    .unwrap();
    let allowed = provider.request(request_value(&allowed_after_denial));
    assert_eq!(allowed["status"], "ok");
    let allowed_response: CustodyProviderResponseV1 =
        serde_json::from_value(allowed["data"].clone()).unwrap();
    assert_eq!(
        allowed_response.status(),
        CustodyProviderResponseStatusV1::Contribution
    );
    let allowed_replay = provider.request(request_value(&allowed_after_denial));
    assert_eq!(allowed_replay["data"], allowed["data"]);

    let wrong_decision = {
        let other_record = provisioning_record();
        let other = signed_release_operation(&other_record);
        signed_decision(&other)
    };
    let bad_release = CustodyProviderRequestV1::new_release_contribution(
        &wrong_runtime_operation,
        &wrong_decision,
    )
    .unwrap();
    let rejected = serde_json::to_string(&provider.request(request_value(&bad_release))).unwrap();
    assert!(rejected.contains("invalid_request"));
    assert!(!rejected.contains("runtime"));
    provider.stop();
}

#[test]
fn custody_provider_process_malformed_shutdown_keeps_serving_until_valid_shutdown() {
    let mut provider = ProviderProcess::start();

    let malformed = provider.request(serde_json::json!({
        "op": "shutdown",
        "unexpected": true
    }));
    assert_eq!(malformed["status"], "error");
    assert_eq!(malformed["code"], "invalid_request");

    let status = provider.request(serde_json::json!({"op": "status"}));
    assert_eq!(status["status"], "ok");
    assert_eq!(status["data"]["provider"], "custody");
    assert_eq!(status["data"]["configured"], false);

    provider.shutdown_and_wait_with_stdin_open();
}
