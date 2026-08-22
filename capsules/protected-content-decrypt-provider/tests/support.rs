use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use ed25519_dalek::{Signer as _, SigningKey};
use elastos_auth::ethereum_signed_message_hash;
use elastos_protected_content_contracts::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationStatementV1,
    CustodyEnvelopeV1, CustodyEpochIdentityV1, CustodyEpochIssuerKeyV1, CustodyEpochStatementV1,
    CustodyNodeIdentityV1, CustodyNodeProvisioningRecordV1, CustodyPoolFailureDomainIdV1,
    CustodyPoolIdentityV1, CustodyPoolMemberStateV1, CustodyPoolMemberV1, CustodyPoolOperatorIdV1,
    CustodyPoolStatementV1, Digest32, EvmContractAddressV1, EvmFunctionSelectorV1,
    EvmRightsMethodAbiV1, KeyReleaseOutcomeV1, KeyReleaseRequestV1, NodeContributionRefV1,
    NodePublicKey, ProtectedContentBindingV1, RecipientKeyAuthorizationStatementV1,
    RecipientKeyIdentityV1, RecipientPublicKeyBytesV1, ReplayNonce16, RightsActionV1,
    RightsDecisionV1, RightsEvaluationEvidenceRequestV1, RightsObservationFinalityV1,
    RightsPolicyBodyV1, RightsRequestV1, RightsSubjectSourceV1, RuntimeCustodyProvisioningIdV1,
    RuntimeCustodyProvisioningStatementV1, RuntimeOperationIssuerKeyV1, RuntimeReleaseAuditIdV1,
    RuntimeReleaseOperationStatementV1, RuntimeSessionBindingV1, ShareCoordinateV1,
    SignedCustodyCommitteeAuthorizationV1, SignedCustodyEpochV1, SignedCustodyPoolV1,
    SignedNodeContributionV1, SignedRecipientKeyAuthorizationV1,
    SignedRuntimeCustodyProvisioningV1, SignedRuntimeReleaseOperationV1, SignedTerminalReceiptV1,
    TerminalReceiptIssuerKey, TerminalReceiptStatementV1, ThresholdV1, ValidatedCustodyCommitteeV1,
    WalletAddress, WalletSignedRightsRequestV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
};
use elastos_protected_content_custody::{
    provision_custody_envelope, ContentEncryptionKeyV1, DurableReplayClaimStoreV1,
    NodeCustodySecretKeyV1, NodeLocalShareStoreV1, RecipientPublicKeyV1,
};
use elastos_protected_content_provider_contracts::CencFmp4MediaIdentityV1;
use k256::ecdsa::SigningKey as WalletSigningKey;
use sha3::{Digest as _, Keccak256};
use tempfile::TempDir;
use zeroize::Zeroizing;

pub const MEDIA_MIME_TYPE_V1: &str = "video/mp4";
pub const MEDIA_CODECS_V1: &str = "avc1.640028,mp4a.40.2";

pub fn issued_at(base_time: u64) -> u64 {
    base_time.saturating_sub(5)
}

pub fn digest(byte: u8) -> Digest32 {
    Digest32::new([byte; 32])
}

pub fn runtime_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
    RuntimeOperationIssuerKeyV1::new(runtime_signing_key(seed).verifying_key().to_bytes()).unwrap()
}

pub fn wallet(seed: u8) -> WalletAddress {
    let key = WalletSigningKey::from_slice(&[seed; 32]).unwrap();
    let encoded = key.verifying_key().to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    WalletAddress::new(digest[12..].try_into().unwrap())
}

pub fn node_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub fn node_public_key(seed: u8) -> NodePublicKey {
    NodePublicKey::new(node_signing_key(seed).verifying_key().to_bytes()).unwrap()
}

pub fn node_custody_secret(seed: u8) -> NodeCustodySecretKeyV1 {
    NodeCustodySecretKeyV1::from_guarded_bytes(Zeroizing::new([seed; 32])).unwrap()
}

fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let size = (8 + content.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(content);
    out
}

fn make_fullbox(box_type: &[u8; 4], flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut content = vec![0u8];
    content.extend_from_slice(&flags.to_be_bytes()[1..]);
    content.extend_from_slice(payload);
    make_box(box_type, &content)
}

fn make_sinf(original_fourcc: &[u8; 4]) -> Vec<u8> {
    let frma = make_box(b"frma", original_fourcc);
    let mut schm_payload = Vec::new();
    schm_payload.extend_from_slice(b"cenc");
    schm_payload.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let schm = make_fullbox(b"schm", 0, &schm_payload);
    let mut tenc_payload = vec![0, 0, 1, 8];
    tenc_payload.extend_from_slice(&[0x44; 16]);
    let tenc = make_fullbox(b"tenc", 0, &tenc_payload);
    let schi = make_box(b"schi", &tenc);
    let mut sinf_content = Vec::new();
    sinf_content.extend_from_slice(&frma);
    sinf_content.extend_from_slice(&schm);
    sinf_content.extend_from_slice(&schi);
    make_box(b"sinf", &sinf_content)
}

fn make_track(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
    let mut tkhd_payload = vec![0u8; 12];
    tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
    let tkhd = make_fullbox(b"tkhd", 0, &tkhd_payload);
    let mut hdlr_payload = vec![0u8; 4];
    hdlr_payload.extend_from_slice(handler_type);
    let hdlr = make_fullbox(b"hdlr", 0, &hdlr_payload);
    let (entry_type, orig_type, fixed) = match handler_type {
        b"vide" => (b"encv", b"avc1", 78usize),
        b"soun" => (b"enca", b"mp4a", 28usize),
        _ => panic!("unsupported handler"),
    };
    let mut entry_content = vec![0u8; fixed];
    entry_content.extend_from_slice(&make_sinf(orig_type));
    let entry = make_box(entry_type, &entry_content);
    let mut stsd_payload = vec![0u8; 4];
    stsd_payload.extend_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&entry);
    let stsd = make_box(b"stsd", &stsd_payload);
    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let mut mdia_content = Vec::new();
    mdia_content.extend_from_slice(&hdlr);
    mdia_content.extend_from_slice(&minf);
    let mdia = make_box(b"mdia", &mdia_content);
    let mut trak_content = Vec::new();
    trak_content.extend_from_slice(&tkhd);
    trak_content.extend_from_slice(&mdia);
    make_box(b"trak", &trak_content)
}

fn make_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut tfhd_payload = Vec::new();
    tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
    tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
    tfhd_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
    let tfhd = make_fullbox(b"tfhd", 0x020038, &tfhd_payload);
    let tfdt = make_fullbox(b"tfdt", 0, &1u32.to_be_bytes());
    let mut trun_payload = Vec::new();
    trun_payload.extend_from_slice(&1u32.to_be_bytes());
    trun_payload.extend_from_slice(&0i32.to_be_bytes());
    trun_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let trun = make_fullbox(b"trun", 0x000201, &trun_payload);
    let mut senc_payload = Vec::new();
    senc_payload.extend_from_slice(&1u32.to_be_bytes());
    senc_payload.extend_from_slice(&(0x10u64 + u64::from(track_id)).to_be_bytes());
    let senc = make_fullbox(b"senc", 0, &senc_payload);
    let mut traf_content = Vec::new();
    traf_content.extend_from_slice(&tfhd);
    traf_content.extend_from_slice(&tfdt);
    traf_content.extend_from_slice(&trun);
    traf_content.extend_from_slice(&senc);
    let traf = make_box(b"traf", &traf_content);
    let mfhd = make_fullbox(b"mfhd", 0, &1u32.to_be_bytes());
    let mut moof_content = Vec::new();
    moof_content.extend_from_slice(&mfhd);
    moof_content.extend_from_slice(&traf);
    let mut moof = make_box(b"moof", &moof_content);
    let data_offset = (moof.len() + 8) as i32;
    let trun_offset = moof
        .windows(4)
        .position(|window| window == b"trun")
        .unwrap()
        - 4;
    let trun_data_offset_at = trun_offset + 16;
    moof[trun_data_offset_at..trun_data_offset_at + 4].copy_from_slice(&data_offset.to_be_bytes());
    let mdat = make_box(b"mdat", payload);
    let mut out = moof;
    out.extend_from_slice(&mdat);
    out
}

pub fn media_components(seed: u8) -> (Vec<u8>, Vec<Vec<u8>>, &'static str, &'static str) {
    let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
    let trak_video = make_track(1, b"vide");
    let trak_audio = make_track(2, b"soun");
    let trex_video = make_fullbox(
        b"trex",
        0,
        &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let trex_audio = make_fullbox(
        b"trex",
        0,
        &[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    let mut mvex_content = Vec::new();
    mvex_content.extend_from_slice(&trex_video);
    mvex_content.extend_from_slice(&trex_audio);
    let mvex = make_box(b"mvex", &mvex_content);
    let mvhd = make_box(b"mvhd", &[0u8; 4]);
    let mut moov_content = Vec::new();
    moov_content.extend_from_slice(&mvhd);
    moov_content.extend_from_slice(&trak_video);
    moov_content.extend_from_slice(&trak_audio);
    moov_content.extend_from_slice(&mvex);
    let moov = make_box(b"moov", &moov_content);
    let encrypted_segments = [0usize, 1]
        .into_iter()
        .map(|index| {
            let track_id = if index % 2 == 0 { 1 } else { 2 };
            let payload = vec![
                seed,
                track_id as u8,
                (index & 0xff) as u8,
                ((index >> 8) & 0xff) as u8,
                b's',
                b'e',
                b'g',
                b'x',
            ];
            make_segment(track_id, &payload)
        })
        .collect();
    (
        [ftyp, moov].concat(),
        encrypted_segments,
        MEDIA_MIME_TYPE_V1,
        MEDIA_CODECS_V1,
    )
}

pub fn media_identity(seed: u8) -> CencFmp4MediaIdentityV1 {
    let (init_segment, encrypted_segments, mime_type, codecs) = media_components(seed);
    CencFmp4MediaIdentityV1::new_from_bytes(&init_segment, &encrypted_segments, mime_type, codecs)
        .unwrap()
}

pub fn policy_body() -> RightsPolicyBodyV1 {
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

fn custody_pool_member(node_seed: u8, custody_seed: u8, base_time: u64) -> CustodyPoolMemberV1 {
    CustodyPoolMemberV1::new(
        node_public_key(node_seed),
        node_custody_secret(custody_seed).public_key().unwrap(),
        CustodyPoolOperatorIdV1::new([0x80 + node_seed; 32]),
        CustodyPoolFailureDomainIdV1::new([0x90 + node_seed; 32]),
        CustodyApprovedSuitesV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        )
        .unwrap(),
        (issued_at(base_time), base_time + 600),
        CustodyPoolMemberStateV1::Active,
    )
    .unwrap()
}

pub fn signed_custody_pool(base_time: u64) -> SignedCustodyPoolV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let statement = CustodyPoolStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        vec![
            custody_pool_member(1, 1, base_time),
            custody_pool_member(2, 2, base_time),
            custody_pool_member(3, 3, base_time),
        ],
    )
    .unwrap();
    SignedCustodyPoolV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub fn signed_custody_epoch() -> SignedCustodyEpochV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let statement = CustodyEpochStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        CustodyApprovedSuitesV1::new(
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
            CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        )
        .unwrap(),
        ThresholdV1::new(2, 3).unwrap(),
        vec![
            CustodyNodeIdentityV1::new(
                node_public_key(1),
                node_custody_secret(1).public_key().unwrap(),
                ShareCoordinateV1::new(1).unwrap(),
            )
            .unwrap(),
            CustodyNodeIdentityV1::new(
                node_public_key(2),
                node_custody_secret(2).public_key().unwrap(),
                ShareCoordinateV1::new(2).unwrap(),
            )
            .unwrap(),
            CustodyNodeIdentityV1::new(
                node_public_key(3),
                node_custody_secret(3).public_key().unwrap(),
                ShareCoordinateV1::new(3).unwrap(),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    SignedCustodyEpochV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub fn signed_committee_authorization(
    pool_identity: CustodyPoolIdentityV1,
    epoch_identity: CustodyEpochIdentityV1,
) -> SignedCustodyCommitteeAuthorizationV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let statement = CustodyCommitteeAuthorizationStatementV1::new(
        CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        pool_identity,
        epoch_identity,
    )
    .unwrap();
    SignedCustodyCommitteeAuthorizationV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub fn validated_custody_committee(base_time: u64) -> ValidatedCustodyCommitteeV1 {
    let issuer_key = SigningKey::from_bytes(&[0x71; 32]);
    let issuer = CustodyEpochIssuerKeyV1::new(issuer_key.verifying_key().to_bytes()).unwrap();
    let pool = signed_custody_pool(base_time);
    let epoch = signed_custody_epoch();
    let authorization = signed_committee_authorization(
        pool.pool_identity().unwrap(),
        epoch.epoch_identity().unwrap(),
    );
    elastos_protected_content_contracts::validate_custody_epoch_against_pool_at(
        issuer,
        authorization.authorization_identity().unwrap(),
        &pool,
        &epoch,
        &authorization,
        base_time,
    )
    .unwrap()
}

pub fn custody_envelope_for_media(seed: u8, base_time: u64) -> CustodyEnvelopeV1 {
    let encrypted_content = media_identity(seed).encrypted_content().clone();
    let content_key = ContentEncryptionKeyV1::generate().unwrap();
    provision_custody_envelope(
        encrypted_content,
        &content_key,
        &validated_custody_committee(base_time),
    )
    .unwrap()
}

pub fn binding_for_envelope(envelope: &CustodyEnvelopeV1) -> ProtectedContentBindingV1 {
    let policy = policy_body();
    ProtectedContentBindingV1::new(
        envelope.manifest().encrypted_content().clone(),
        envelope.key_envelope_identity().unwrap(),
        policy.policy_identity().unwrap(),
        elastos_protected_content_contracts::ProfileIdentityV1::from_public_key_bytes(
            SigningKey::from_bytes(&[0x26; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        wallet(7),
        RuntimeSessionBindingV1::new(digest(0x66)).unwrap(),
    )
    .unwrap()
}

pub fn make_signed_runtime_release_operation(
    runtime_seed: u8,
    audit_request_id: RuntimeReleaseAuditIdV1,
    envelope: &CustodyEnvelopeV1,
    recipient_public_key: RecipientPublicKeyBytesV1,
    recipient_identity: RecipientKeyIdentityV1,
    base_time: u64,
) -> SignedRuntimeReleaseOperationV1 {
    let runtime_key = runtime_signing_key(runtime_seed);
    let binding = binding_for_envelope(envelope);
    let rights_request = {
        let request = RightsRequestV1::new(
            binding.clone(),
            RightsActionV1::View,
            recipient_identity.clone(),
            issued_at(base_time),
            base_time + 180,
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
    };
    let release_request = KeyReleaseRequestV1::new(
        binding.clone(),
        rights_request.request().request_hash().unwrap(),
        RightsActionV1::View,
        recipient_identity.clone(),
        issued_at(base_time),
        base_time + 50,
        ReplayNonce16::new([0x66; 16]),
    )
    .unwrap();
    let profile = SigningKey::from_bytes(&[0x26; 32]);
    let authorization_statement = RecipientKeyAuthorizationStatementV1::new(
        binding.clone(),
        RightsActionV1::View,
        recipient_public_key,
        recipient_identity,
        runtime_issuer(runtime_seed),
        issued_at(base_time),
        base_time + 180,
    )
    .unwrap();
    let authorization = SignedRecipientKeyAuthorizationV1::new(
        authorization_statement.clone(),
        profile
            .sign(&authorization_statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let policy = policy_body();
    let evidence_request =
        RightsEvaluationEvidenceRequestV1::new(binding.clone(), policy.policy_identity().unwrap())
            .unwrap();
    let statement = RuntimeReleaseOperationStatementV1::new(
        runtime_issuer(runtime_seed),
        rights_request,
        release_request,
        recipient_public_key,
        authorization,
        policy,
        evidence_request,
        signed_custody_epoch(),
        audit_request_id,
        issued_at(base_time),
        base_time + 40,
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

fn make_signed_node_rights_decision(
    operation: &SignedRuntimeReleaseOperationV1,
    node_seed: u8,
    base_time: u64,
) -> elastos_protected_content_contracts::SignedNodeRightsDecisionV1 {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), base_time)
        .unwrap();
    let statement = elastos_protected_content_contracts::NodeRightsDecisionStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.rights_request_hash(),
        authenticated.binding().clone(),
        authenticated.action(),
        node_public_key(node_seed),
        RightsDecisionV1::Allowed,
        digest(0x80 ^ node_seed),
        issued_at(base_time),
        base_time + 50,
    )
    .unwrap();
    elastos_protected_content_contracts::SignedNodeRightsDecisionV1::new(
        statement.clone(),
        node_signing_key(node_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

fn signed_runtime_provisioning_for_record(
    record: &CustodyNodeProvisioningRecordV1,
    runtime_seed: u8,
    base_time: u64,
) -> SignedRuntimeCustodyProvisioningV1 {
    let statement = RuntimeCustodyProvisioningStatementV1::new(
        runtime_issuer(runtime_seed),
        record.record_identity().unwrap(),
        RuntimeCustodyProvisioningIdV1::new(digest(
            0xc0 ^ record.selected_node_public_key().as_bytes()[0],
        ))
        .unwrap(),
        issued_at(base_time),
        base_time + 30,
    )
    .unwrap();
    SignedRuntimeCustodyProvisioningV1::new(
        statement.clone(),
        runtime_signing_key(runtime_seed)
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

pub fn tighten_tempdir_permissions(tempdir: &TempDir) {
    #[cfg(unix)]
    {
        fs::set_permissions(tempdir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
}

pub fn make_signed_node_contribution(
    operation: &SignedRuntimeReleaseOperationV1,
    envelope: &CustodyEnvelopeV1,
    runtime_seed: u8,
    node_seed: u8,
    base_time: u64,
) -> SignedNodeContributionV1 {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), base_time)
        .unwrap();
    let decision = make_signed_node_rights_decision(operation, node_seed, base_time);
    let record = CustodyNodeProvisioningRecordV1::new(
        envelope.key_envelope_identity().unwrap(),
        envelope.manifest().clone(),
        node_public_key(node_seed),
        envelope
            .stored_share_for_node(node_public_key(node_seed))
            .unwrap()
            .clone(),
    )
    .unwrap();
    let signed_provisioning =
        signed_runtime_provisioning_for_record(&record, runtime_seed, base_time);
    let node_secret = node_custody_secret(node_seed);
    let tempdir = TempDir::new().unwrap();
    tighten_tempdir_permissions(&tempdir);
    let node_share_store = NodeLocalShareStoreV1::new(
        node_public_key(node_seed),
        tempdir.path().join("share-store"),
    );
    let provisioned = node_share_store
        .provision_node_share(
            &record,
            &signed_provisioning,
            runtime_issuer(runtime_seed),
            &node_secret,
            base_time,
        )
        .unwrap();
    let recipient_public_key =
        RecipientPublicKeyV1::new(*operation.statement().recipient_public_key().as_bytes())
            .unwrap();
    let mut replay_store = DurableReplayClaimStoreV1::new(
        node_public_key(node_seed),
        tempdir.path().join("replay-store"),
    );
    replay_store
        .claim_or_replay_node_contribution(
            authenticated,
            &decision,
            provisioned.node_share(),
            &node_signing_key(node_seed),
            &node_secret,
            &recipient_public_key,
            issued_at(base_time),
            base_time + 40,
            base_time,
        )
        .unwrap()
}

pub fn make_signed_terminal_receipt(
    operation: &SignedRuntimeReleaseOperationV1,
    contributions: &[SignedNodeContributionV1],
    issuer_seed: u8,
    base_time: u64,
) -> SignedTerminalReceiptV1 {
    let authenticated = operation
        .verify(operation.statement().runtime_operation_issuer(), base_time)
        .unwrap();
    let node_set = signed_custody_epoch().statement().node_set().unwrap();
    let verified_contributions = contributions
        .iter()
        .map(|contribution| {
            authenticated
                .verify_node_contribution(contribution, &node_set, base_time)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let refs = verified_contributions
        .iter()
        .map(NodeContributionRefV1::from)
        .collect::<Vec<_>>();
    let issuer_key = SigningKey::from_bytes(&[issuer_seed; 32]);
    let statement = TerminalReceiptStatementV1::new(
        authenticated.release_request_hash(),
        authenticated.binding().clone(),
        TerminalReceiptIssuerKey::new(issuer_key.verifying_key().to_bytes()).unwrap(),
        KeyReleaseOutcomeV1::Released,
        refs,
        issued_at(base_time),
        base_time + 40,
    )
    .unwrap();
    SignedTerminalReceiptV1::new(
        statement.clone(),
        issuer_key
            .sign(&statement.canonical_bytes().unwrap())
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}
