use super::*;

fn provider_body(value: serde_json::Value) -> Body {
    Body::from(serde_json::to_vec(&value).unwrap())
}

async fn set_mock_wallet_transaction_default(
    wallet_provider: &MockWalletProvider,
    principal_id: &str,
    chain_namespace: &str,
    account_id: &str,
    set_at: u64,
) {
    let mut defaults = wallet_provider.defaults.lock().await;
    defaults.push(json!({
        "schema": "elastos.wallet.default_account/v1",
        "principal_id": principal_id,
        "chain_namespace": chain_namespace,
        "intent": "transaction_intent",
        "account_id": account_id,
        "set_at": set_at,
    }));
}

fn protected_content_gateway_mock_test_guard() -> &'static tokio::sync::Mutex<()> {
    static GUARD: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD.get_or_init(|| tokio::sync::Mutex::new(()))
}

const ELACITY_PLAYER_CAPSULE_ID_FOR_TEST: &str = "elacity-player";

async fn post_library(
    app: axum::Router,
    token: &str,
    op: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/provider/object/{op}"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(provider_body(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            panic!(
                "invalid json response for {op} (status={status}): {err}; body={}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
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

fn make_clear_track(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
    let mut tkhd_payload = vec![0u8; 12];
    tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
    let tkhd = make_fullbox(b"tkhd", 0, &tkhd_payload);
    let mut hdlr_payload = vec![0u8; 4];
    hdlr_payload.extend_from_slice(handler_type);
    let hdlr = make_fullbox(b"hdlr", 0, &hdlr_payload);
    let (entry_type, fixed) = match handler_type {
        b"vide" => (b"avc1", 78usize),
        b"soun" => (b"mp4a", 28usize),
        _ => panic!("unsupported handler"),
    };
    let entry = make_box(entry_type, &vec![0u8; fixed]);
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

fn make_clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
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
    let mut traf_content = Vec::new();
    traf_content.extend_from_slice(&tfhd);
    traf_content.extend_from_slice(&tfdt);
    traf_content.extend_from_slice(&trun);
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
        .expect("trun box present")
        - 4;
    let trun_data_offset_at = trun_offset + 16;
    moof[trun_data_offset_at..trun_data_offset_at + 4].copy_from_slice(&data_offset.to_be_bytes());
    let mdat = make_box(b"mdat", payload);
    let mut out = moof;
    out.extend_from_slice(&mdat);
    out
}

fn clear_runtime_custody_media(seed: u8) -> (Vec<u8>, Vec<Vec<u8>>) {
    let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
    let trak_video = make_clear_track(1, b"vide");
    let trak_audio = make_clear_track(2, b"soun");
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
    let clear_segments = [0usize, 1]
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
            make_clear_segment(track_id, &payload)
        })
        .collect();
    ([ftyp, moov].concat(), clear_segments)
}

fn library_object_path(data_dir: &std::path::Path, uri: &str) -> std::path::PathBuf {
    elastos_common::localhost::rooted_localhost_fs_path(data_dir, uri).unwrap()
}

fn library_publish_records_dir(
    data_dir: &std::path::Path,
    principal_id: &str,
) -> std::path::PathBuf {
    let root = crate::auth::principal_localhost_root(principal_id);
    library_object_path(
        data_dir,
        &format!("{root}/.AppData/LocalHost/.Runtime/Library/Published"),
    )
}

fn assert_no_publish_records(data_dir: &std::path::Path, principal_id: &str) {
    let records_dir = library_publish_records_dir(data_dir, principal_id);
    if records_dir.exists() {
        assert!(std::fs::read_dir(records_dir).unwrap().next().is_none());
    }
}

fn runtime_custody_creator_test_input(
    principal_id: &str,
    object_uri: &str,
    seed: u8,
    wallet_account_id: &str,
) -> crate::protected_content_runtime::RuntimeCustodyLibraryPublishInput {
    let (clear_init_segment, clear_segments) = clear_runtime_custody_media(seed);
    let wallet_account_address = wallet_account_id
        .rsplit(':')
        .next()
        .filter(|value| value.starts_with("0x") && value.len() == 42)
        .unwrap_or("0x1111111111111111111111111111111111111111")
        .to_string();
    crate::protected_content_runtime::RuntimeCustodyLibraryPublishInput {
        object_uri: object_uri.to_string(),
        principal_id: principal_id.to_string(),
        mime_type: "video/mp4".to_string(),
        codecs: "avc1.64001f,mp4a.40.2".to_string(),
        wallet_account_id: wallet_account_id.to_string(),
        wallet_account_address,
        creator_mint_source_digest: runtime_custody_creator_source_digest(),
        copies: "0x2".to_string(),
        price: MOCK_PROTECTED_CONTENT_LISTING_PRICE.to_string(),
        clear_init_segment,
        clear_segments,
        source_storage: "protected_principal_root".to_string(),
    }
}

fn runtime_custody_creator_source_digest() -> elastos_protected_content_contracts::Digest32 {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"elastos.runtime-custody.creator-mint-source/v1");
    for field in [
        "base-mainnet",
        "eip155:8453",
        MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY,
        MOCK_PROTECTED_CONTENT_PAY_TOKEN,
        "elacity_mint_v1",
        "mint(string,uint16,bytes,bytes)",
    ] {
        hasher.update((field.len() as u32).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    elastos_protected_content_contracts::Digest32::new(hasher.finalize().into())
}

fn seed_completed_runtime_custody_mint(
    data_dir: &std::path::Path,
    input: &crate::protected_content_runtime::RuntimeCustodyLibraryPublishInput,
) -> crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
    let protected_content_root = data_dir.join("protected-content");
    std::fs::create_dir_all(&protected_content_root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            &protected_content_root,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let journal = crate::protected_content_runtime::runtime_mint_journal(data_dir);
    let (protected_init_segment, protected_segments) =
        crate::protected_content_runtime::tests::media_components(0x41);
    let node_public_key = |seed: u8| {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        elastos_protected_content_contracts::NodePublicKey::new(
            signing_key.verifying_key().to_bytes(),
        )
        .unwrap()
    };
    let node_bindings = vec![
        elastos_protected_content_runtime::RuntimeMintNodeBinding::new(
            node_public_key(0x11),
            elastos_protected_content_contracts::CustodyPoolOperatorIdV1::new([0x21; 32]),
            elastos_protected_content_contracts::CustodyPoolFailureDomainIdV1::new([0x31; 32]),
            elastos_protected_content_contracts::Digest32::new([0x41; 32]),
        )
        .unwrap(),
        elastos_protected_content_runtime::RuntimeMintNodeBinding::new(
            node_public_key(0x12),
            elastos_protected_content_contracts::CustodyPoolOperatorIdV1::new([0x22; 32]),
            elastos_protected_content_contracts::CustodyPoolFailureDomainIdV1::new([0x32; 32]),
            elastos_protected_content_contracts::Digest32::new([0x42; 32]),
        )
        .unwrap(),
        elastos_protected_content_runtime::RuntimeMintNodeBinding::new(
            node_public_key(0x13),
            elastos_protected_content_contracts::CustodyPoolOperatorIdV1::new([0x23; 32]),
            elastos_protected_content_contracts::CustodyPoolFailureDomainIdV1::new([0x33; 32]),
            elastos_protected_content_contracts::Digest32::new([0x43; 32]),
        )
        .unwrap(),
    ];
    let content_access_id =
        elastos_protected_content_contracts::ContentAccessIdV1::new([0x51; 16]).unwrap();
    let media_identity =
        elastos_protected_content_provider_contracts::CencFmp4MediaIdentityV1::new_from_bytes(
            &protected_init_segment,
            &protected_segments,
            input.mime_type.clone(),
            input.codecs.clone(),
        )
        .unwrap();
    let threshold = elastos_protected_content_contracts::ThresholdV1::new(2, 3).unwrap();
    let node_set = elastos_protected_content_contracts::NodeSetV1::new(
        threshold,
        node_bindings
            .iter()
            .map(|node| node.node_public_key())
            .collect(),
    )
    .unwrap();
    let key_envelope = elastos_protected_content_contracts::KeyEnvelopeIdentityV1::new(
        media_identity.encrypted_content().clone(),
        elastos_protected_content_contracts::Digest32::new([0x52; 32]),
        256,
        node_set.node_set_id().unwrap(),
        threshold,
        elastos_protected_content_contracts::CustodyPoolIdentityV1::new(
            elastos_protected_content_contracts::Digest32::new([0x53; 32]),
            128,
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyEpochIdentityV1::new(
            elastos_protected_content_contracts::Digest32::new([0x54; 32]),
            128,
        )
        .unwrap(),
        elastos_protected_content_contracts::CustodyCommitteeAuthorizationIdentityV1::new(
            elastos_protected_content_contracts::Digest32::new([0x55; 32]),
            128,
        )
        .unwrap(),
    )
    .unwrap();
    let policy = elastos_protected_content_contracts::RightsPolicyIdentityV1::new(
        elastos_protected_content_contracts::Digest32::new([0x56; 32]),
        128,
    )
    .unwrap();
    let draft = elastos_protected_content_runtime::RuntimeMintDraft::new(
        &protected_init_segment,
        &protected_segments,
        input.mime_type.clone(),
        input.codecs.clone(),
        content_access_id,
        key_envelope,
        policy,
        elastos_protected_content_contracts::Digest32::new([0x61; 32]),
        threshold,
        node_bindings.clone(),
    )
    .unwrap();
    journal.persist_bound(&draft).unwrap();
    let request_id = elastos_protected_content_runtime::RuntimeMintIntent::request_id_for_source(
        &input.principal_id,
        &input.object_uri,
        &input.source_storage,
    )
    .unwrap();
    let intent = elastos_protected_content_runtime::RuntimeMintIntent::new(
        input.principal_id.clone(),
        &input.object_uri,
        &input.source_storage,
        input.wallet_account_id.clone(),
        input.wallet_account_address.clone(),
        input.creator_mint_source_digest,
        input.mime_type.clone(),
        input.codecs.clone(),
        &input.clear_init_segment,
        &input.clear_segments,
        draft.content_access_id(),
        draft.pool(),
        draft.epoch(),
        draft.committee(),
        node_bindings.clone(),
    )
    .unwrap();
    journal.persist_intent(&intent).unwrap();
    journal
        .mark_intent_protect_effect_started(request_id)
        .unwrap();
    journal
        .mark_intent_protect_closed_before_draft(request_id)
        .unwrap();
    for (index, node) in node_bindings.iter().enumerate() {
        journal
            .mark_node_effect_started(draft.mint_id(), node.node_public_key())
            .unwrap();
        journal
            .mark_node_receipt(
                draft.mint_id(),
                elastos_protected_content_runtime::RuntimeMintNodeReceipt::new(
                    node.node_public_key(),
                    elastos_protected_content_contracts::RuntimeCustodyProvisioningIdV1::new(
                        elastos_protected_content_contracts::Digest32::new([0x71 + index as u8; 32]),
                    )
                    .unwrap(),
                    elastos_protected_content_contracts::CustodyNodeProvisioningRecordIdentityV1::new(
                        elastos_protected_content_contracts::Digest32::new([0x81 + index as u8; 32]),
                        128 + index as u32,
                    )
                    .unwrap(),
                    node.owner_state_root(),
                )
                .unwrap(),
            )
            .unwrap();
    }
    journal.mark_custody_provisioned(draft.mint_id()).unwrap();
    let content_id =
        crate::protected_content_runtime::runtime_protected_content_id(draft.encrypted_content())
            .unwrap();
    let requirement =
        elastos_protected_content_runtime::RuntimeContentAvailabilityRequirement::new(
            mock_protected_content_provider_signer_did(),
            content_id.clone(),
            input.principal_id.clone(),
            "protected-content-replication/v1",
            3,
            600,
            60,
        )
        .unwrap();
    let evidence = elastos_protected_content_runtime::RuntimeVerifiedContentAvailability::new(
        TEST_CIDV1,
        content_id.clone(),
        input.principal_id.clone(),
        &requirement,
        3,
        crate::auth::now_ts(),
        elastos_protected_content_contracts::Digest32::new([0xa1; 32]),
        draft.encrypted_content().clone(),
        draft.media_identity().media_manifest_root(),
    )
    .unwrap();
    journal
        .mark_content_available(draft.mint_id(), &requirement, evidence.clone())
        .unwrap();
    seed_mock_published_protected_content(
        &content_id,
        &input.principal_id,
        draft.media_identity(),
        &protected_init_segment,
        &protected_segments,
        evidence.checked_at(),
    );
    journal
        .mark_intent_completed(request_id, draft.mint_id())
        .unwrap();
    crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: TEST_CIDV1.to_string(),
        mint_id: draft.mint_id(),
        content_id,
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({
            "status": "local_pinned",
            "provider": "mock-content-provider",
            "replicas": 3
        }),
        receipt: json!({
            "schema": "elastos.content.availability.receipt/v1",
            "cid": TEST_CIDV1
        }),
        content_security: json!({
            "mode": "runtime_custody",
            "access": "buyer_purchase_required"
        }),
    }
}

fn seed_runtime_custody_creator_listing_for_buy(
    data_dir: &std::path::Path,
    publisher_principal_id: &str,
    facts: &crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts,
    seller_address: &str,
    native_purchase: bool,
) -> crate::protected_content_runtime::RuntimeCustodyListingRecord {
    let mint = crate::protected_content_runtime::runtime_mint_journal(data_dir)
        .load(facts.mint_id)
        .unwrap();
    let metadata_cid = TEST_CIDV0;
    let token_uri = format!("ipfs://{metadata_cid}/metadata.json");
    let pay_token = if native_purchase {
        "0x0000000000000000000000000000000000000000".to_string()
    } else {
        MOCK_PROTECTED_CONTENT_PAY_TOKEN.to_ascii_lowercase()
    };
    let terminal = elastos_protected_content_runtime::RuntimeMintCreatorTerminalEvidence::new(
        metadata_cid,
        &token_uri,
        seller_address.to_ascii_lowercase(),
        "eip155:8453",
        "base-mainnet",
        MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY.to_ascii_lowercase(),
        MOCK_PROTECTED_CONTENT_TOKEN_ID,
        MOCK_PROTECTED_CONTENT_OPERATIVE.to_ascii_lowercase(),
        if native_purchase { "0x1" } else { "0x2" },
        MOCK_PROTECTED_CONTENT_LISTING_PRICE,
        pay_token,
        (!native_purchase).then_some(MOCK_PROTECTED_CONTENT_PAYMENT_PROCESSOR.to_ascii_lowercase()),
        format!("0x{}", hex::encode([0xc1; 32])),
        crate::auth::now_ts(),
    )
    .unwrap();
    crate::protected_content_runtime::persist_runtime_custody_creator_listing(
        data_dir,
        &mint,
        facts,
        publisher_principal_id,
        &terminal,
    )
    .unwrap();
    crate::protected_content_runtime::load_runtime_custody_listing(data_dir, facts.mint_id)
        .unwrap()
        .unwrap()
}

fn runtime_custody_listing_path_for_test(
    data_dir: &std::path::Path,
    mint_id: elastos_protected_content_contracts::Digest32,
) -> std::path::PathBuf {
    data_dir
        .join("protected-content/runtime-listings")
        .join(format!("{}.json", hex::encode(mint_id.as_bytes())))
}

fn runtime_custody_viewer_record_path_for_test(
    data_dir: &std::path::Path,
    principal_id: &str,
    mint_id: elastos_protected_content_contracts::Digest32,
) -> std::path::PathBuf {
    data_dir
        .join("protected-content/runtime-open")
        .join(hex::encode(mint_id.as_bytes()))
        .join("viewers")
        .join(format!(
            "{}.json",
            hex::encode(sha2::Sha256::digest(principal_id.as_bytes()))
        ))
}

async fn write_library_bytes(app: &axum::Router, token: &str, uri: &str, bytes: &[u8]) {
    let (status, payload) = post_library(
        app.clone(),
        token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "uri={uri} payload={payload}");
    assert_eq!(payload["status"], "ok", "uri={uri} payload={payload}");
}

async fn create_runtime_custody_publish_directory(
    data_dir: &std::path::Path,
    app: &axum::Router,
    token: &str,
    directory_uri: &str,
    seed: u8,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let directory_path = library_object_path(data_dir, directory_uri);
    std::fs::create_dir_all(directory_path.join("segments")).unwrap();
    let (init, segments) = clear_runtime_custody_media(seed);
    write_library_bytes(app, token, &format!("{directory_uri}/init.mp4"), &init).await;
    for (index, segment) in segments.iter().enumerate() {
        write_library_bytes(
            app,
            token,
            &format!("{directory_uri}/segments/{index:08}.m4s"),
            segment,
        )
        .await;
    }
    (init, segments)
}

async fn publish_runtime_custody(
    app: &axum::Router,
    token: &str,
    uri: &str,
) -> (StatusCode, serde_json::Value) {
    post_library(
        app.clone(),
        token,
        "publish",
        json!({
            "uri": uri,
            "protection": {
                "mode": "runtime_custody",
                "copies": "0x1",
                "price": "0xde0b6b3a7640000"
            }
        }),
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "test helper binds every fixture fact explicitly"
)]
async fn assert_rejected_runtime_custody_protection_field(
    data_dir: &std::path::Path,
    app: &axum::Router,
    token: &str,
    principal_id: &str,
    uri: &str,
    field: &str,
    value: &str,
    expected_message: &str,
) {
    let mut protection = serde_json::Map::new();
    protection.insert(
        "mode".into(),
        serde_json::Value::String("runtime_custody".into()),
    );
    protection.insert("copies".into(), serde_json::Value::String("0x1".into()));
    protection.insert(
        "price".into(),
        serde_json::Value::String("0xde0b6b3a7640000".into()),
    );
    protection.insert(field.into(), serde_json::Value::String(value.to_string()));
    let (publish_status, publish) = post_library(
        app.clone(),
        token,
        "publish",
        json!({
            "uri": uri,
            "protection": serde_json::Value::Object(protection),
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);
    assert_eq!(publish["status"], "error");
    let message = publish["message"].as_str().unwrap();
    assert!(
        message.contains(expected_message),
        "expected `{expected_message}` in `{message}`"
    );
    assert!(!message.contains(uri));
    assert!(!message.contains(&data_dir.display().to_string()));
    assert_no_publish_records(data_dir, principal_id);
}

async fn assert_runtime_custody_publish_error(
    data_dir: &std::path::Path,
    app: &axum::Router,
    token: &str,
    principal_id: &str,
    uri: &str,
    expected_message: &str,
) -> serde_json::Value {
    let (status, payload) = publish_runtime_custody(app, token, uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    let message = payload["message"].as_str().unwrap();
    assert_eq!(message, expected_message);
    let payload_text = payload.to_string();
    assert!(!payload_text.contains(uri));
    assert!(!payload_text.contains(&data_dir.display().to_string()));
    assert!(!payload_text.contains("segx"));
    assert_no_publish_records(data_dir, principal_id);
    payload
}

async fn put_library_upload(
    app: axum::Router,
    token: &str,
    uri: &str,
    body: &'static [u8],
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(format!("/api/provider/object/upload?uri={encoded_uri}"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "text/plain")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, payload)
}

async fn post_library_upload_start(
    app: axum::Router,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/upload/start")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(provider_body(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, payload)
}

async fn put_library_upload_chunk(
    app: axum::Router,
    token: &str,
    upload_id: &str,
    offset: u64,
    body: &'static [u8],
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(format!("/api/provider/object/upload/{upload_id}/chunk"))
                .header("x-elastos-home-token", token)
                .header("x-elastos-upload-offset", offset.to_string())
                .header(CONTENT_TYPE, "text/plain")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, payload)
}

async fn post_library_upload_finish(
    app: axum::Router,
    token: &str,
    upload_id: &str,
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("/api/provider/object/upload/{upload_id}/finish"))
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, payload)
}

async fn get_library_download(
    app: axum::Router,
    token: &str,
    uri: &str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    get_library_download_with_range(app, token, uri, None).await
}

async fn get_library_download_with_range(
    app: axum::Router,
    token: &str,
    uri: &str,
    range: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");
    let mut request = test_browser_request("localhost:61180", "null")
        .method("GET")
        .uri(format!(
            "/api/provider/object/download/raw?uri={encoded_uri}"
        ))
        .header("x-elastos-home-token", token);
    if let Some(range) = range {
        request = request.header(axum::http::header::RANGE, range);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

async fn get_library_download_many(
    app: axum::Router,
    token: &str,
    uris: &[String],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    get_library_download_many_with_archive(app, token, uris, None).await
}

async fn get_library_download_many_with_archive(
    app: axum::Router,
    token: &str,
    uris: &[String],
    archive: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut query_parts = uris
        .iter()
        .map(|uri| {
            let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");
            format!("uri={encoded_uri}")
        })
        .collect::<Vec<_>>();
    if let Some(archive) = archive {
        query_parts.push(format!("archive={archive}"));
    }
    let query = query_parts.join("&");
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!("/api/provider/object/download/raw?{query}"))
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

fn transfer_receipt(headers: &HeaderMap) -> serde_json::Value {
    serde_json::from_str(
        headers
            .get("x-elastos-transfer-receipt")
            .and_then(|value| value.to_str().ok())
            .unwrap(),
    )
    .unwrap()
}

fn zip_text_files(bytes: &[u8]) -> std::collections::BTreeMap<String, String> {
    use std::io::Read as _;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut files = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut body = String::new();
        entry.read_to_string(&mut body).unwrap();
        files.insert(name, body);
    }
    files
}

#[tokio::test]
async fn test_library_provider_requires_library_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/roots")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let documents_token = issue_home_launch_token(dir.path(), DOCUMENTS_CAPSULE_ID).unwrap();
    let rejected = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/roots")
                .header("x-elastos-home-token", documents_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_library_download_route_requires_library_token_and_streams_download_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let documents_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/raw-download.txt");
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "mime": "text/plain",
            "data": base64::engine::general_purpose::STANDARD.encode(b"raw download body"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/provider/object/download/raw?uri={encoded_uri}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let (rejected_status, _headers, _bytes) =
        get_library_download(app.clone(), &documents_token, &uri).await;
    assert_eq!(rejected_status, StatusCode::FORBIDDEN);

    let (download_status, headers, bytes) = get_library_download(app.clone(), &token, &uri).await;
    assert_eq!(download_status, StatusCode::OK);
    assert_eq!(bytes, b"raw download body");
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/plain"),
    );
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"raw-download.txt\""),
    );
    assert_eq!(
        headers
            .get(axum::http::header::ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok()),
        Some("bytes"),
    );
    assert!(headers
        .get("x-elastos-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .starts_with("object:download:"));
    let receipt = transfer_receipt(&headers);
    assert_eq!(receipt["schema"], "elastos.object.transfer.receipt/v1");
    assert_eq!(receipt["op"], "download");
    assert_eq!(receipt["status"], "completed");
    assert_eq!(receipt["bytes"], 17);
    assert_eq!(receipt["total_bytes"], 17);
    assert_eq!(receipt["transport"], "http-body-stream");
    assert_eq!(
        receipt["stream"]["schema"],
        "elastos.object.download-stream/v1"
    );
    assert_eq!(receipt["stream"]["mode"], "response_body_chunks");
    assert_eq!(receipt["stream"]["backpressure"], "http_body_poll");
    assert_eq!(receipt["stream"]["cancel"], "drop_body");

    let (range_status, range_headers, range_bytes) =
        get_library_download_with_range(app, &token, &uri, Some("bytes=4-11")).await;
    assert_eq!(range_status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(range_bytes, b"download");
    assert_eq!(
        range_headers
            .get(axum::http::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok()),
        Some("bytes 4-11/17"),
    );
    assert_eq!(
        range_headers
            .get(axum::http::header::ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok()),
        Some("bytes"),
    );
    let range_receipt = transfer_receipt(&range_headers);
    assert_eq!(
        range_receipt["schema"],
        "elastos.object.transfer.receipt/v1"
    );
    assert_eq!(range_receipt["op"], "download");
    assert_eq!(range_receipt["status"], "completed");
    assert_eq!(range_receipt["bytes"], 8);
    assert_eq!(range_receipt["total_bytes"], 17);
    assert_eq!(range_receipt["range"]["start"], 4);
    assert_eq!(range_receipt["range"]["end"], 11);
    assert_eq!(range_receipt["transport"], "http-body-stream");
    assert_eq!(range_receipt["stream"]["mode"], "response_body_chunks");
}

#[tokio::test]
async fn test_library_upload_route_requires_library_token_and_writes_raw_body() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let documents_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/raw-upload.txt");
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(format!("/api/provider/object/upload?uri={encoded_uri}"))
                .header(CONTENT_TYPE, "text/plain")
                .body(Body::from("no token"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(format!("/api/provider/object/upload?uri={encoded_uri}"))
                .header("x-elastos-home-token", documents_token)
                .header(CONTENT_TYPE, "text/plain")
                .body(Body::from("wrong app"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let (upload_status, upload_headers, upload) =
        put_library_upload(app.clone(), &token, &uri, b"raw upload body").await;
    assert_eq!(upload_status, StatusCode::OK);
    assert_eq!(upload["status"], "ok");
    assert_eq!(upload["data"]["transport"], "raw-body");
    assert_eq!(upload["data"]["object"]["uri"], uri);
    assert!(upload["data"]["object"]["content_cid"]
        .as_str()
        .unwrap()
        .starts_with("bafkrei"));
    assert_eq!(upload["data"]["object"].get("published_cid"), None);
    assert_eq!(upload["data"]["object"]["published"], false);
    assert!(upload["data"]["request_id"]
        .as_str()
        .unwrap()
        .starts_with("object:upload:"));
    assert_eq!(
        upload["data"]["receipt"]["schema"],
        "elastos.object.transfer.receipt/v1"
    );
    assert_eq!(upload["data"].get("provider_receipt"), None);
    assert_eq!(upload["data"]["receipt"]["op"], "upload");
    assert_eq!(upload["data"]["receipt"]["status"], "completed");
    assert_eq!(upload["data"]["receipt"]["bytes"], 15);
    assert_eq!(upload["data"]["receipt"]["total_bytes"], 15);
    assert_eq!(
        upload_headers
            .get("x-elastos-request-id")
            .and_then(|value| value.to_str().ok()),
        upload["data"]["request_id"].as_str()
    );
    assert_eq!(
        transfer_receipt(&upload_headers)["schema"],
        "elastos.object.transfer.receipt/v1"
    );

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let data = read["data"]["data"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        b"raw upload body"
    );
}

#[tokio::test]
async fn test_library_chunked_upload_session_writes_object_and_emits_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/chunked-upload.txt");
    let body = b"chunked upload body";

    let (start_status, start) = post_library_upload_start(
        app.clone(),
        &token,
        json!({
            "uri": uri,
            "mime": "text/plain",
            "size_bytes": body.len(),
        }),
    )
    .await;
    assert_eq!(start_status, StatusCode::OK);
    assert_eq!(start["status"], "ok");
    assert_eq!(start["data"]["schema"], "elastos.object.upload-session/v1");
    assert_eq!(start["data"]["transport"], "http-chunk-session");
    assert_eq!(start["data"]["received_bytes"], 0);
    assert!(start["data"]["chunk_size"].as_u64().unwrap() < 1024 * 1024);
    let upload_id = start["data"]["upload_id"].as_str().unwrap();

    let (first_status, first_chunk) =
        put_library_upload_chunk(app.clone(), &token, upload_id, 0, b"chunked ").await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first_chunk["data"]["received_bytes"], 8);
    assert_eq!(first_chunk["data"]["chunk_count"], 1);

    let (second_status, second_chunk) =
        put_library_upload_chunk(app.clone(), &token, upload_id, 8, b"upload body").await;
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_chunk["data"]["received_bytes"], body.len());
    assert_eq!(second_chunk["data"]["chunk_count"], 2);

    let (finish_status, finish_headers, finish) =
        post_library_upload_finish(app.clone(), &token, upload_id).await;
    assert_eq!(finish_status, StatusCode::OK);
    assert_eq!(finish["status"], "ok");
    assert_eq!(finish["data"]["object"]["uri"], uri);
    assert!(finish["data"]["object"]["content_cid"]
        .as_str()
        .unwrap()
        .starts_with("bafkrei"));
    assert_eq!(finish["data"]["object"].get("published_cid"), None);
    assert_eq!(finish["data"]["object"]["published"], false);
    assert_eq!(finish["data"]["transport"], "raw-body");
    assert_eq!(finish["data"]["browser_transport"], "http-chunk-session");
    assert_eq!(finish["data"]["upload_session"]["chunk_count"], 2);
    assert_eq!(finish["data"]["receipt"]["op"], "upload");
    assert_eq!(finish["data"]["receipt"]["transport"], "http-chunk-session");
    assert_eq!(
        finish["data"]["receipt"]["stream"]["backpressure"],
        "client_waits_for_chunk_ack"
    );
    assert_eq!(
        transfer_receipt(&finish_headers)["transport"],
        "http-chunk-session"
    );

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let data = read["data"]["data"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        body
    );
}

#[tokio::test]
async fn test_documents_viewer_route_can_read_and_save_library_file_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let library_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let documents_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/from-library.md");
    write_test_static_capsule(
        dir.path(),
        DOCUMENTS_CAPSULE_ID,
        "viewer",
        "Test Documents viewer",
        "<!doctype html><title>Documents Viewer</title>",
    );
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");

    let (write_status, write) = post_library(
        app.clone(),
        &library_token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"# From Library"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let direct_read = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/read")
                .header("x-elastos-home-token", documents_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "uri": uri,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_read.status(), StatusCode::FORBIDDEN);

    let read = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/documents/library-object?uri={encoded_uri}"
                ))
                .header("x-elastos-home-token", documents_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let read_body = axum::body::to_bytes(read.into_body(), usize::MAX)
        .await
        .unwrap();
    let read: serde_json::Value = serde_json::from_slice(&read_body).unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, b"# From Library");

    let revision = read["data"]["object"]["revision"].as_str().unwrap();
    let save = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(format!(
                    "/api/viewers/documents/library-object?uri={encoded_uri}"
                ))
                .header("x-elastos-home-token", documents_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "if_revision": revision,
                        "data": base64::engine::general_purpose::STANDARD
                            .encode(b"# Saved From Documents"),
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(save.status(), StatusCode::OK);
    let save_body = axum::body::to_bytes(save.into_body(), usize::MAX)
        .await
        .unwrap();
    let save: serde_json::Value = serde_json::from_slice(&save_body).unwrap();
    assert_eq!(save["status"], "ok");
}

#[tokio::test]
async fn test_library_provider_rejects_unknown_operation() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();

    let rejected = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/raw_host_path")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_gateway_provider_proxy_rejects_predeclared_runtime_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();

    for reserved in [
        "_runtime_invocation",
        "_runtime_transfer",
        "connect_ticket",
        "carrier_route",
        "carrier",
    ] {
        let rejected = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/provider/object/roots")
                    .header("x-elastos-home-token", token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&json!({
                            reserved: { "schema": "spoofed" }
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
            .await
            .unwrap();
        let message = String::from_utf8(body.to_vec()).unwrap();
        assert!(message.contains("provider request must not predeclare Runtime metadata field"));
        assert!(message.contains(reserved));
    }
}

#[tokio::test]
async fn test_library_provider_object_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");

    let (roots_status, roots) = post_library(app.clone(), &token, "roots", json!({})).await;
    assert_eq!(roots_status, StatusCode::OK);
    assert_eq!(roots["status"], "ok");
    assert!(roots["data"]["roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|root| root["id"] == "documents" && root["label"] == "Documents"));
    assert!(roots["data"]["roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|root| root["id"] == "desktop" && root["label"] == "Desktop"));
    assert!(roots["data"]["roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|root| {
            root["id"] == "webspaces"
                && root["label"] == "Spaces"
                && root["uri"] == "localhost://WebSpaces"
        }));
    assert!(roots["data"]["roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["id"] == "trash"
                && entry["label"] == "Trash"
                && entry["uri"] == format!("{root}/.Trash")
                && entry["metadata"]["empty"] == true
        }));

    let (mkdir_status, mkdir) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    assert_eq!(mkdir["status"], "ok");
    assert_eq!(mkdir["data"]["object"]["uri"], documents_uri);

    let notes_uri = format!("{documents_uri}/notes.txt");
    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": notes_uri,
            "mime": "text/plain",
            "data": base64::engine::general_purpose::STANDARD.encode(b"hello library"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");
    assert_eq!(write["data"]["object"]["name"], "notes.txt");

    let (list_status, list) = post_library(
        app.clone(),
        &token,
        "list",
        json!({
            "uri": documents_uri,
        }),
    )
    .await;
    assert_eq!(list_status, StatusCode::OK);
    assert!(list["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| object["name"] == "notes.txt" && object["kind"] == "file"));

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": notes_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let data = read["data"]["data"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        b"hello library"
    );

    let (rename_status, rename) = post_library(
        app.clone(),
        &token,
        "rename",
        json!({
            "uri": notes_uri,
            "name": "renamed.txt",
        }),
    )
    .await;
    assert_eq!(rename_status, StatusCode::OK);
    let renamed_uri = rename["data"]["object"]["uri"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(renamed_uri.ends_with("/Documents/renamed.txt"));

    let (trash_status, trash) = post_library(
        app.clone(),
        &token,
        "trash",
        json!({
            "uri": renamed_uri,
        }),
    )
    .await;
    assert_eq!(trash_status, StatusCode::OK);
    let trash_uri = trash["data"]["object"]["uri"].as_str().unwrap().to_string();
    assert!(trash_uri.contains("/.Trash/"));
    assert_eq!(
        trash["data"]["object"]["metadata"]["trash"]["original_uri"],
        renamed_uri
    );

    let (restore_status, restore) = post_library(
        app.clone(),
        &token,
        "restore",
        json!({
            "uri": trash_uri,
        }),
    )
    .await;
    assert_eq!(restore_status, StatusCode::OK);
    assert_eq!(restore["data"]["object"]["uri"], renamed_uri);

    let (trash_again_status, trash_again) = post_library(
        app.clone(),
        &token,
        "trash",
        json!({
            "uri": renamed_uri,
        }),
    )
    .await;
    assert_eq!(trash_again_status, StatusCode::OK);
    let deleted_uri = trash_again["data"]["object"]["uri"].as_str().unwrap();
    let (delete_status, deleted) = post_library(
        app.clone(),
        &token,
        "delete_permanently",
        json!({
            "uri": deleted_uri,
        }),
    )
    .await;
    assert_eq!(delete_status, StatusCode::OK);
    assert_eq!(deleted["data"]["deleted_uri"], deleted_uri);

    let cleanup_uri = format!("{documents_uri}/cleanup.txt");
    let (cleanup_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": cleanup_uri,
            "mime": "text/plain",
            "data": base64::engine::general_purpose::STANDARD.encode(b"cleanup"),
        }),
    )
    .await;
    assert_eq!(cleanup_status, StatusCode::OK);
    let (trash_cleanup_status, _) = post_library(
        app.clone(),
        &token,
        "trash",
        json!({
            "uri": cleanup_uri,
        }),
    )
    .await;
    assert_eq!(trash_cleanup_status, StatusCode::OK);
    let (empty_status, empty) = post_library(app, &token, "empty_trash", json!({})).await;
    assert_eq!(empty_status, StatusCode::OK);
    assert_eq!(empty["data"]["deleted_count"], 1);
}

#[tokio::test]
async fn test_library_provider_separates_public_placement_from_publish_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let public_uri = format!("{root}/Public");

    let (mkdir_status, mkdir) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Public",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    assert_eq!(
        mkdir["data"]["object"]["metadata"]["visibility"]["schema"],
        "elastos.library.visibility/v1"
    );
    assert_eq!(
        mkdir["data"]["object"]["metadata"]["visibility"]["placement"],
        "public_folder"
    );
    assert_eq!(
        mkdir["data"]["object"]["metadata"]["visibility"]["effective_access"],
        "principal_private"
    );

    let file_uri = format!("{public_uri}/draft.txt");
    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": file_uri,
            "mime": "text/plain",
            "data": base64::engine::general_purpose::STANDARD.encode(b"public placement draft"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");
    assert_eq!(
        write["data"]["object"]["metadata"]["visibility"]["schema"],
        "elastos.library.visibility/v1"
    );
    assert_eq!(
        write["data"]["object"]["metadata"]["visibility"]["placement"],
        "public_folder"
    );
    assert_eq!(
        write["data"]["object"]["metadata"]["visibility"]["effective_access"],
        "principal_private"
    );
    assert_eq!(
        write["data"]["object"]["metadata"]["visibility"]["publish_required_for_public_link"],
        true
    );
    assert_eq!(write["data"]["object"]["published"], false);
    assert!(write["data"]["object"]["published_cid"].is_null());
    assert!(write["data"]["object"]["metadata"]["visibility"]["published_cid"].is_null());

    let (publish_status, publish) = post_library(
        app,
        &token,
        "publish",
        json!({
            "uri": file_uri,
            "if_revision": write["data"]["object"]["revision"],
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);
    assert_eq!(publish["status"], "ok");
    assert_eq!(publish["data"]["cid"], TEST_CIDV1);
    assert_eq!(publish["data"]["uri"], format!("elastos://{TEST_CIDV1}"));
    assert_eq!(
        publish["data"]["object"]["metadata"]["visibility"]["placement"],
        "public_folder"
    );
    assert_eq!(
        publish["data"]["object"]["metadata"]["visibility"]["effective_access"],
        "public_content_link"
    );
    assert_eq!(
        publish["data"]["object"]["metadata"]["visibility"]["publish_required_for_public_link"],
        false
    );
    assert_eq!(publish["data"]["object"]["published"], true);
    assert_eq!(publish["data"]["object"]["published_cid"], TEST_CIDV1);
    assert_eq!(
        publish["data"]["object"]["metadata"]["visibility"]["published_cid"],
        TEST_CIDV1
    );
    assert_eq!(
        publish["data"]["object"]["metadata"]["visibility"]["published_link"],
        format!("elastos://{TEST_CIDV1}")
    );
}

#[tokio::test]
async fn test_library_provider_downloads_directory_archive() {
    use std::io::Read as _;

    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let nested_uri = format!("{documents_uri}/Nested");

    let (mkdir_status, mkdir) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    assert!(mkdir["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "download"));

    let (root_stat_status, root_stat) = post_library(
        app.clone(),
        &token,
        "stat",
        json!({
            "uri": root,
        }),
    )
    .await;
    assert_eq!(root_stat_status, StatusCode::OK);
    assert!(!root_stat["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "download"));

    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    for (uri, bytes) in [
        (
            format!("{documents_uri}/notes.txt"),
            b"folder archive".as_slice(),
        ),
        (
            format!("{nested_uri}/deep.txt"),
            b"nested archive".as_slice(),
        ),
    ] {
        let (write_status, write) = post_library(
            app.clone(),
            &token,
            "write",
            json!({
                "uri": uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
        assert_eq!(write_status, StatusCode::OK);
        assert_eq!(write["status"], "ok");
    }

    let (download_status, download) = post_library(
        app,
        &token,
        "download",
        json!({
            "uri": documents_uri,
        }),
    )
    .await;
    assert_eq!(download_status, StatusCode::OK);
    assert_eq!(download["data"]["filename"], "Documents.tar.gz");
    assert_eq!(download["data"]["object"]["mime"], "application/gzip");
    let archive_bytes = base64::engine::general_purpose::STANDARD
        .decode(download["data"]["data"].as_str().unwrap())
        .unwrap();
    let decoder = flate2::read::GzDecoder::new(archive_bytes.as_slice());
    let mut archive = tar::Archive::new(decoder);
    let mut files = std::collections::BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().unwrap().to_string_lossy().to_string();
        let mut body = String::new();
        entry.read_to_string(&mut body).unwrap();
        files.insert(path, body);
    }
    assert_eq!(
        files.get("Documents/notes.txt").map(String::as_str),
        Some("folder archive")
    );
    assert_eq!(
        files.get("Documents/Nested/deep.txt").map(String::as_str),
        Some("nested archive")
    );
}

#[tokio::test]
async fn test_library_download_route_archives_selected_objects() {
    use std::io::Read as _;

    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let nested_uri = format!("{documents_uri}/Nested");
    let alpha_uri = format!("{documents_uri}/alpha.txt");
    let deep_uri = format!("{nested_uri}/deep.txt");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    for (uri, bytes) in [
        (alpha_uri.clone(), b"selected alpha".as_slice()),
        (deep_uri, b"selected nested".as_slice()),
    ] {
        let (write_status, write) = post_library(
            app.clone(),
            &token,
            "write",
            json!({
                "uri": uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
        assert_eq!(write_status, StatusCode::OK);
        assert_eq!(write["status"], "ok");
    }

    let selected = vec![alpha_uri, nested_uri];
    let (download_status, headers, archive_bytes) =
        get_library_download_many(app, &token, &selected).await;
    assert_eq!(download_status, StatusCode::OK);
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/gzip"),
    );
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"Documents Selection.tar.gz\""),
    );
    let receipt = transfer_receipt(&headers);
    assert_eq!(receipt["schema"], "elastos.object.transfer.receipt/v1");
    assert_eq!(receipt["op"], "download");
    assert_eq!(receipt["status"], "completed");
    assert_eq!(receipt["uri"], "selection:2");

    let decoder = flate2::read::GzDecoder::new(archive_bytes.as_slice());
    let mut archive = tar::Archive::new(decoder);
    let mut files = std::collections::BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().unwrap().to_string_lossy().to_string();
        let mut body = String::new();
        entry.read_to_string(&mut body).unwrap();
        files.insert(path, body);
    }
    assert_eq!(
        files.get("alpha.txt").map(String::as_str),
        Some("selected alpha")
    );
    assert_eq!(
        files.get("Nested/deep.txt").map(String::as_str),
        Some("selected nested")
    );
}

#[tokio::test]
async fn test_library_download_route_archives_directory_as_zip() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let nested_uri = format!("{documents_uri}/Nested");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    for (uri, bytes) in [
        (
            format!("{documents_uri}/alpha.txt"),
            b"zip alpha".as_slice(),
        ),
        (format!("{nested_uri}/deep.txt"), b"zip nested".as_slice()),
    ] {
        let (write_status, write) = post_library(
            app.clone(),
            &token,
            "write",
            json!({
                "uri": uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
        assert_eq!(write_status, StatusCode::OK);
        assert_eq!(write["status"], "ok");
    }

    let (download_status, headers, archive_bytes) =
        get_library_download_many_with_archive(app, &token, &[documents_uri], Some("zip")).await;
    assert_eq!(download_status, StatusCode::OK);
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/zip"),
    );
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"Documents.zip\""),
    );
    let files = zip_text_files(&archive_bytes);
    assert_eq!(
        files.get("Documents/alpha.txt").map(String::as_str),
        Some("zip alpha")
    );
    assert_eq!(
        files.get("Documents/Nested/deep.txt").map(String::as_str),
        Some("zip nested")
    );
}

#[tokio::test]
async fn test_library_download_route_archives_selected_objects_as_zip() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let nested_uri = format!("{documents_uri}/Nested");
    let alpha_uri = format!("{documents_uri}/alpha.txt");
    let deep_uri = format!("{nested_uri}/deep.txt");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    for (uri, bytes) in [
        (alpha_uri.clone(), b"selected zip alpha".as_slice()),
        (deep_uri, b"selected zip nested".as_slice()),
    ] {
        let (write_status, write) = post_library(
            app.clone(),
            &token,
            "write",
            json!({
                "uri": uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
        assert_eq!(write_status, StatusCode::OK);
        assert_eq!(write["status"], "ok");
    }

    let selected = vec![alpha_uri, nested_uri];
    let (download_status, headers, archive_bytes) =
        get_library_download_many_with_archive(app, &token, &selected, Some("zip")).await;
    assert_eq!(download_status, StatusCode::OK);
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/zip"),
    );
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment; filename=\"Documents Selection.zip\""),
    );
    let receipt = transfer_receipt(&headers);
    assert_eq!(receipt["schema"], "elastos.object.transfer.receipt/v1");
    assert_eq!(receipt["op"], "download");
    assert_eq!(receipt["status"], "completed");
    assert_eq!(receipt["uri"], "selection:2");

    let files = zip_text_files(&archive_bytes);
    assert_eq!(
        files.get("alpha.txt").map(String::as_str),
        Some("selected zip alpha")
    );
    assert_eq!(
        files.get("Nested/deep.txt").map(String::as_str),
        Some("selected zip nested")
    );
}

#[tokio::test]
async fn test_library_download_route_rejects_unknown_archive_format() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let (download_status, _headers, body) =
        get_library_download_many_with_archive(app, &token, &[documents_uri], Some("rar")).await;
    assert_eq!(download_status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8(body)
        .unwrap()
        .contains("unsupported Library archive format: rar"));
}

#[tokio::test]
async fn test_library_provider_marks_generic_archive_families_policy_gated() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let archive_token = app_token_for_authority(dir.path(), "archive-manager", &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/Bundle.7z");
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");
    write_test_static_capsule(
        dir.path(),
        "archive-manager",
        "viewer",
        "Test Archive viewer",
        "<!doctype html><title>Archive</title>",
    );
    write_test_static_capsule(
        dir.path(),
        "documents",
        "viewer",
        "Test Documents viewer",
        "<!doctype html><title>Documents</title>",
    );

    let (write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"not a real 7z archive"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);

    let (stat_status, stat) = post_library(
        app.clone(),
        &token,
        "stat",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(stat_status, StatusCode::OK);
    let object = &stat["data"]["object"];
    assert_eq!(
        object["metadata"]["archive_support"]["schema"],
        "elastos.library.archive-support/v1"
    );
    assert_eq!(object["metadata"]["archive_support"]["family"], "7z");
    assert_eq!(
        object["metadata"]["archive_support"]["status"],
        "policy_gated_unsupported_archive_family"
    );
    assert!(!object["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "extract_archive"));
    assert_eq!(object["viewer"], "archive-manager");
    assert_eq!(object["viewers"][0]["id"], "archive-manager");

    let direct_read = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/read")
                .header("x-elastos-home-token", archive_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "uri": uri,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_read.status(), StatusCode::FORBIDDEN);

    let viewer_stat = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_uri}&stat_only=true"
                ))
                .header("x-elastos-home-token", archive_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_stat.status(), StatusCode::OK);
    let viewer_body = axum::body::to_bytes(viewer_stat.into_body(), usize::MAX)
        .await
        .unwrap();
    let viewer_stat: serde_json::Value = serde_json::from_slice(&viewer_body).unwrap();
    assert_eq!(
        viewer_stat["data"]["object"]["metadata"]["archive_support"]["status"],
        "policy_gated_unsupported_archive_family"
    );
    assert!(viewer_stat["data"].get("data").is_none());

    let viewer_read = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_uri}"
                ))
                .header(
                    "x-elastos-home-token",
                    app_token_for_authority(dir.path(), "archive-manager", &authority),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_read.status(), StatusCode::FORBIDDEN);

    let (extract_status, extract) = post_library(
        app.clone(),
        &token,
        "extract_archive",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(extract_status, StatusCode::OK);
    assert_eq!(extract["status"], "error");
    assert!(extract["message"]
        .as_str()
        .unwrap()
        .contains("only supports .tar, .tar.gz, .tgz, and .zip"));

    let (entries_status, entries) = post_library(
        app,
        &token,
        "archive_entries",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(entries_status, StatusCode::OK);
    assert_eq!(entries["status"], "error");
    assert!(entries["message"]
        .as_str()
        .unwrap()
        .contains("archive listing only supports .tar, .tar.gz, .tgz, and .zip"));
}

#[tokio::test]
async fn test_library_provider_compresses_folder_to_zip_object() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let projects_uri = format!("{documents_uri}/Projects");
    let nested_uri = format!("{projects_uri}/Nested");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (projects_status, projects) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Projects",
        }),
    )
    .await;
    assert_eq!(projects_status, StatusCode::OK);
    assert!(projects["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "compress_archive"));
    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": projects_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    for (uri, bytes) in [
        (format!("{projects_uri}/alpha.txt"), b"zip alpha".as_slice()),
        (format!("{nested_uri}/deep.txt"), b"zip nested".as_slice()),
    ] {
        let (write_status, write) = post_library(
            app.clone(),
            &token,
            "write",
            json!({
                "uri": uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
        assert_eq!(write_status, StatusCode::OK);
        assert_eq!(write["status"], "ok");
        assert!(write["data"]["object"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "compress_archive"));
    }

    let (compress_status, compressed) = post_library(
        app.clone(),
        &token,
        "compress_archive",
        json!({
            "uri": projects_uri,
        }),
    )
    .await;
    assert_eq!(compress_status, StatusCode::OK);
    assert_eq!(compressed["status"], "ok");
    assert_eq!(
        compressed["data"]["object"]["uri"],
        format!("{documents_uri}/Projects.zip")
    );
    assert_eq!(compressed["data"]["object"]["mime"], "application/zip");
    assert!(compressed["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "extract_archive"));

    let (compress_again_status, compressed_again) = post_library(
        app.clone(),
        &token,
        "compress_archive",
        json!({
            "uri": projects_uri,
        }),
    )
    .await;
    assert_eq!(compress_again_status, StatusCode::OK);
    let second_zip_uri = compressed_again["data"]["object"]["uri"].as_str().unwrap();
    assert_ne!(second_zip_uri, format!("{documents_uri}/Projects.zip"));
    assert!(second_zip_uri.starts_with(&format!("{documents_uri}/Projects (")));
    assert!(second_zip_uri.ends_with(").zip"));

    let zip_uri = compressed["data"]["object"]["uri"].as_str().unwrap();
    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let archive_bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    let files = zip_text_files(&archive_bytes);
    assert_eq!(
        files.get("Projects/alpha.txt").map(String::as_str),
        Some("zip alpha")
    );
    assert_eq!(
        files.get("Projects/Nested/deep.txt").map(String::as_str),
        Some("zip nested")
    );

    let (events_status, events) = post_library(
        app,
        &token,
        "events",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(events_status, StatusCode::OK);
    assert!(events["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["op"] == "compress_archive"));
}

#[tokio::test]
async fn test_library_provider_stores_incompressible_zip_entries() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let video_uri = format!("{documents_uri}/Screen Recording.mp4");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let video_bytes = vec![0x5a; 2048];
    let (write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": video_uri,
            "mime": "video/mp4",
            "data": base64::engine::general_purpose::STANDARD.encode(&video_bytes),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    let (compress_status, compressed) = post_library(
        app.clone(),
        &token,
        "compress_archive",
        json!({
            "uri": video_uri,
        }),
    )
    .await;
    assert_eq!(compress_status, StatusCode::OK);
    let zip_uri = compressed["data"]["object"]["uri"].as_str().unwrap();
    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let archive_bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(archive_bytes)).unwrap();
    let mut entry = archive.by_name("Screen Recording.mp4").unwrap();
    assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    let mut roundtrip = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut roundtrip).unwrap();
    assert_eq!(roundtrip, video_bytes);
}

#[tokio::test]
async fn test_library_provider_compresses_selected_objects_to_zip_object() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let nested_uri = format!("{documents_uri}/Nested");
    let alpha_uri = format!("{documents_uri}/alpha.txt");
    let deep_uri = format!("{nested_uri}/deep.txt");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    for (uri, bytes) in [
        (alpha_uri.clone(), b"selected zip alpha".as_slice()),
        (deep_uri, b"selected zip nested".as_slice()),
    ] {
        let (write_status, write) = post_library(
            app.clone(),
            &token,
            "write",
            json!({
                "uri": uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
        assert_eq!(write_status, StatusCode::OK);
        assert_eq!(write["status"], "ok");
    }

    let (compress_status, compressed) = post_library(
        app.clone(),
        &token,
        "compress_archive",
        json!({
            "uris": [alpha_uri, nested_uri],
        }),
    )
    .await;
    assert_eq!(compress_status, StatusCode::OK);
    assert_eq!(compressed["status"], "ok");
    assert_eq!(
        compressed["data"]["object"]["uri"],
        format!("{documents_uri}/Documents Selection.zip")
    );
    assert_eq!(compressed["data"]["object"]["mime"], "application/zip");

    let zip_uri = compressed["data"]["object"]["uri"].as_str().unwrap();
    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let archive_bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    let files = zip_text_files(&archive_bytes);
    assert_eq!(
        files.get("alpha.txt").map(String::as_str),
        Some("selected zip alpha")
    );
    assert_eq!(
        files.get("Nested/deep.txt").map(String::as_str),
        Some("selected zip nested")
    );
}

#[tokio::test]
async fn test_library_provider_extracts_tar_gz_archive() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let archive_uri = format!("{documents_uri}/Bundle.tar.gz");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut alpha = b"extracted alpha".as_slice();
    let mut alpha_header = tar::Header::new_gnu();
    alpha_header.set_size(alpha.len() as u64);
    alpha_header.set_mode(0o644);
    alpha_header.set_cksum();
    builder
        .append_data(&mut alpha_header, "alpha.txt", &mut alpha)
        .unwrap();
    let mut deep = b"extracted nested".as_slice();
    let mut deep_header = tar::Header::new_gnu();
    deep_header.set_size(deep.len() as u64);
    deep_header.set_mode(0o644);
    deep_header.set_cksum();
    builder
        .append_data(&mut deep_header, "Nested/deep.txt", &mut deep)
        .unwrap();
    let archive_bytes = builder.into_inner().unwrap().finish().unwrap();

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": archive_uri,
            "mime": "application/gzip",
            "data": base64::engine::general_purpose::STANDARD.encode(archive_bytes),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["data"]["object"]["mime"], "application/gzip");
    assert!(write["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "extract_archive"));

    let (extract_status, extract) = post_library(
        app.clone(),
        &token,
        "extract_archive",
        json!({
            "uri": archive_uri,
        }),
    )
    .await;
    assert_eq!(extract_status, StatusCode::OK);
    let extracted_uri = extract["data"]["object"]["uri"].as_str().unwrap();
    assert_eq!(extracted_uri, format!("{documents_uri}/Bundle"));

    for (path, expected) in [
        ("alpha.txt", "extracted alpha"),
        ("Nested/deep.txt", "extracted nested"),
    ] {
        let (read_status, read) = post_library(
            app.clone(),
            &token,
            "read",
            json!({
                "uri": format!("{extracted_uri}/{path}"),
            }),
        )
        .await;
        assert_eq!(read_status, StatusCode::OK);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(read["data"]["data"].as_str().unwrap())
            .unwrap();
        let body = String::from_utf8(bytes).unwrap();
        assert_eq!(body, expected);
    }
}

#[tokio::test]
async fn test_library_provider_extracts_plain_tar_archive() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let archive_uri = format!("{documents_uri}/Bundle.tar");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let mut builder = tar::Builder::new(Vec::new());
    let mut alpha = b"plain tar alpha".as_slice();
    let mut alpha_header = tar::Header::new_gnu();
    alpha_header.set_size(alpha.len() as u64);
    alpha_header.set_mode(0o644);
    alpha_header.set_cksum();
    builder
        .append_data(&mut alpha_header, "alpha.txt", &mut alpha)
        .unwrap();
    let archive_bytes = builder.into_inner().unwrap();

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": archive_uri,
            "mime": "application/x-tar",
            "data": base64::engine::general_purpose::STANDARD.encode(archive_bytes),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["data"]["object"]["mime"], "application/x-tar");
    assert!(write["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "extract_archive"));

    let (extract_status, extract) = post_library(
        app.clone(),
        &token,
        "extract_archive",
        json!({
            "uri": archive_uri,
        }),
    )
    .await;
    assert_eq!(extract_status, StatusCode::OK);
    let extracted_uri = extract["data"]["object"]["uri"].as_str().unwrap();
    assert_eq!(extracted_uri, format!("{documents_uri}/Bundle"));

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": format!("{extracted_uri}/alpha.txt"),
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    let body = String::from_utf8(bytes).unwrap();
    assert_eq!(body, "plain tar alpha");
}

#[tokio::test]
async fn test_library_gateway_lists_webspaces_through_runtime_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let (root_status, root) = post_library(
        app.clone(),
        &token,
        "list",
        json!({
            "uri": "localhost://WebSpaces",
        }),
    )
    .await;
    assert_eq!(root_status, StatusCode::OK);
    assert_eq!(root["status"], "ok");
    assert_eq!(root["data"]["uri"], "localhost://WebSpaces");
    let root_objects = root["data"]["objects"].as_array().unwrap();
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    assert!(root_objects.iter().any(|object| {
        object["uri"] == localhost_root
            && object["name"] == "Localhost"
            && object["kind"] == "directory"
            && object["availability"] == "local-principal"
            && object["metadata"]["schema"] == "elastos.library.space-pointer/v1"
            && object["metadata"]["space"] == "localhost"
            && object["metadata"]["target_uri"] == localhost_root
            && object["metadata"]["provider"] == "object-provider"
            && object["metadata"]["authority"] == "signed-principal-root"
            && object["metadata"]["writable"] == true
            && object["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "list")
    }));
    assert!(root_objects.iter().any(|object| {
        object["uri"] == "localhost://WebSpaces/Elastos"
            && object["kind"] == "directory"
            && object["availability"] == "resolver-owned"
            && object["metadata"]["schema"] == "elastos.library.webspace-object/v1"
            && object["metadata"]["mount"] == "Elastos"
            && object["metadata"]["resolver"] == "builtin"
            && object["metadata"]["cache_policy"] == "metadata-only"
            && object["metadata"]["sync_policy"] == "manual"
            && object["metadata"]["object_id"] == "object:webspace:elastos"
            && object["metadata"]["head_id"] == "head:webspace:elastos"
            && object["metadata"]["cache_state"] == "metadata_cached"
            && object["metadata"]["sync_state"] == "manual_idle"
            && object["metadata"]["webspace_kind"] == "dynamic-webspace"
            && object["metadata"]["readonly"] == true
            && object["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "list")
    }));
    let cloud = root_objects
        .iter()
        .find(|object| object["uri"] == "localhost://WebSpaces/Cloud")
        .expect("indexed Cloud WebSpace mount should be listed");
    assert_eq!(cloud["kind"], "directory");
    assert_eq!(cloud["metadata"]["target_uri"], "cloud://drive");
    assert_eq!(cloud["metadata"]["resolver"], "cloud-drive");
    assert_eq!(cloud["metadata"]["cache_policy"], "metadata-and-thumbnails");
    assert_eq!(cloud["metadata"]["webspace_kind"], "mounted-webspace");

    let (elastos_status, elastos) = post_library(
        app.clone(),
        &token,
        "list",
        json!({
            "uri": "localhost://WebSpaces/Elastos",
        }),
    )
    .await;
    assert_eq!(elastos_status, StatusCode::OK);
    assert_eq!(elastos["status"], "ok");
    let names: Vec<&str> = elastos["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|object| object["name"].as_str())
        .collect();
    for expected in ["_meta.json", "content", "peer", "did"] {
        assert!(
            names.contains(&expected),
            "missing WebSpace child {expected}"
        );
    }
    let content = elastos["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["name"] == "content")
        .expect("content WebSpace child should be listed");
    assert_eq!(content["metadata"]["target_uri"], "elastos://<cid>");
    assert_eq!(content["metadata"]["resolver"], "builtin");
    assert_eq!(content["metadata"]["object_id"], "object:webspace:content");
    assert_eq!(content["metadata"]["head_id"], "head:webspace:content");
    assert_eq!(content["metadata"]["cache_state"], "metadata_cached");
    assert_eq!(content["metadata"]["sync_state"], "manual_idle");
    assert_eq!(content["metadata"]["webspace_kind"], "folder-handle");

    let (cloud_status, cloud_list) = post_library(
        app.clone(),
        &token,
        "list",
        json!({
            "uri": "localhost://WebSpaces/Cloud",
        }),
    )
    .await;
    assert_eq!(cloud_status, StatusCode::OK);
    let cloud_names: Vec<&str> = cloud_list["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|object| object["name"].as_str())
        .collect();
    for expected in ["_meta.json", "Drive", "Shared"] {
        assert!(
            cloud_names.contains(&expected),
            "missing indexed Cloud child {expected}"
        );
    }

    let (project_status, project) = post_library(
        app,
        &token,
        "list",
        json!({
            "uri": "localhost://WebSpaces/Cloud/Drive/Project X",
        }),
    )
    .await;
    assert_eq!(project_status, StatusCode::OK);
    let file = project["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["name"] == "file.pdf")
        .expect("indexed Cloud file should be listed");
    assert_eq!(
        file["metadata"]["target_uri"],
        "cloud://drive/Drive/Project X/file.pdf"
    );
    assert_eq!(file["metadata"]["resolver"], "cloud-drive");
    assert_eq!(file["metadata"]["webspace_kind"], "indexed-file");
    assert_eq!(file["availability"], "resolver-owned");
}

#[tokio::test]
async fn test_library_provider_extracts_zip_archive() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let zip_uri = format!("{documents_uri}/Bundle.zip");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let archive_bytes = {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("alpha.txt", options).unwrap();
        writer.write_all(b"zip alpha").unwrap();
        writer.add_directory("Nested/", options).unwrap();
        writer.start_file("Nested/deep.txt", options).unwrap();
        writer.write_all(b"zip nested").unwrap();
        writer.finish().unwrap().into_inner()
    };

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": zip_uri,
            "mime": "application/zip",
            "data": base64::engine::general_purpose::STANDARD.encode(archive_bytes),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");
    assert_eq!(write["data"]["object"]["mime"], "application/zip");
    assert!(write["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "extract_archive"));

    let (extract_status, extracted) = post_library(
        app.clone(),
        &token,
        "extract_archive",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(extract_status, StatusCode::OK);
    assert_eq!(extracted["status"], "ok");
    let extracted_uri = extracted["data"]["object"]["uri"].as_str().unwrap();
    assert_eq!(extracted_uri, format!("{documents_uri}/Bundle"));

    for (path, expected) in [
        ("alpha.txt", "zip alpha"),
        ("Nested/deep.txt", "zip nested"),
    ] {
        let (read_status, read) = post_library(
            app.clone(),
            &token,
            "read",
            json!({
                "uri": format!("{extracted_uri}/{path}"),
            }),
        )
        .await;
        assert_eq!(read_status, StatusCode::OK);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(read["data"]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), expected);
    }
}

#[tokio::test]
async fn test_library_provider_lists_supported_archive_entries_through_viewer_route() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let archive_token = app_token_for_authority(dir.path(), "archive-manager", &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let zip_uri = format!("{documents_uri}/Bundle.zip");
    let tar_uri = format!("{documents_uri}/Bundle.tar");
    let encoded_zip_uri = zip_uri.replace(':', "%3A").replace('/', "%2F");
    write_test_static_capsule(
        dir.path(),
        "archive-manager",
        "viewer",
        "Test Archive viewer",
        "<!doctype html><title>Archive</title>",
    );
    write_test_static_capsule(
        dir.path(),
        "documents",
        "viewer",
        "Test Documents viewer",
        "<!doctype html><title>Documents</title>",
    );

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let zip_bytes = {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("alpha.txt", options).unwrap();
        writer.write_all(b"zip alpha").unwrap();
        writer.add_directory("Nested/", options).unwrap();
        writer.start_file("Nested/deep.txt", options).unwrap();
        writer.write_all(b"zip nested").unwrap();
        writer.finish().unwrap().into_inner()
    };
    let (zip_write_status, zip_write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": zip_uri,
            "mime": "application/zip",
            "data": base64::engine::general_purpose::STANDARD.encode(zip_bytes),
        }),
    )
    .await;
    assert_eq!(zip_write_status, StatusCode::OK);
    assert_eq!(zip_write["data"]["object"]["viewer"], "archive-manager");

    let (entries_status, entries) = post_library(
        app.clone(),
        &token,
        "archive_entries",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(entries_status, StatusCode::OK);
    assert_eq!(entries["status"], "ok");
    assert_eq!(
        entries["data"]["schema"],
        "elastos.library.archive-entries/v1"
    );
    assert_eq!(entries["data"]["family"], "zip");
    assert_eq!(entries["data"]["limits"]["truncated"], false);
    let entry_rows = entries["data"]["entries"].as_array().unwrap();
    assert!(entry_rows.iter().any(|entry| {
        entry["path"] == "alpha.txt"
            && entry["kind"] == "file"
            && entry["safety"]["status"] == "safe"
            && entry["size"] == 9
            && entry["compressed_size"].as_u64().is_some()
    }));
    assert!(entry_rows
        .iter()
        .any(|entry| { entry["path"] == "Nested" && entry["kind"] == "directory" }));
    assert!(entry_rows.iter().any(|entry| {
        entry["path"] == "Nested/deep.txt" && entry["safety"]["status"] == "safe"
    }));

    let direct_entries = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/archive_entries")
                .header("x-elastos-home-token", archive_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "uri": zip_uri,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_entries.status(), StatusCode::FORBIDDEN);

    let direct_preview = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/archive_preview_entry")
                .header("x-elastos-home-token", archive_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "uri": zip_uri,
                        "entry": "Nested/deep.txt",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_preview.status(), StatusCode::FORBIDDEN);

    let direct_roots = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/roots")
                .header("x-elastos-home-token", archive_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_roots.status(), StatusCode::FORBIDDEN);

    let viewer_entries = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_zip_uri}&entries=true"
                ))
                .header("x-elastos-home-token", archive_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_status = viewer_entries.status();
    let viewer_body = axum::body::to_bytes(viewer_entries.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_body)
    );
    let viewer_entries: serde_json::Value = serde_json::from_slice(&viewer_body).unwrap();
    assert_eq!(
        viewer_entries["data"]["schema"],
        "elastos.library.archive-entries/v1"
    );
    assert!(viewer_entries["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "Nested/deep.txt"));

    let viewer_preview = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_zip_uri}&preview_entry=Nested%2Fdeep.txt"
                ))
                .header("x-elastos-home-token", archive_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_preview_status = viewer_preview.status();
    let viewer_preview_body = axum::body::to_bytes(viewer_preview.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_preview_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_preview_body)
    );
    let viewer_preview: serde_json::Value = serde_json::from_slice(&viewer_preview_body).unwrap();
    assert_eq!(
        viewer_preview["data"]["schema"],
        "elastos.library.archive-preview-entry/v1"
    );
    assert_eq!(viewer_preview["data"]["entry"]["path"], "Nested/deep.txt");
    assert_eq!(viewer_preview["data"]["entry"]["mime"], "text/plain");
    assert_eq!(
        viewer_preview["data"]["entry"]["viewers"][0]["id"],
        "documents"
    );
    assert_eq!(viewer_preview["data"]["preview"]["text"], "zip nested");
    assert_eq!(viewer_preview["data"]["preview"]["truncated"], false);

    let viewer_roots = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri("/api/viewers/archive-manager/library-roots")
                .header("x-elastos-home-token", archive_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_roots_status = viewer_roots.status();
    let viewer_roots_body = axum::body::to_bytes(viewer_roots.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_roots_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_roots_body)
    );
    let viewer_roots: serde_json::Value = serde_json::from_slice(&viewer_roots_body).unwrap();
    assert!(viewer_roots["data"]["roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|root| root["label"] == "Documents" && root["uri"] == documents_uri));

    let tar_bytes = {
        let mut builder = tar::Builder::new(Vec::new());
        let mut alpha = b"tar alpha".as_slice();
        let mut alpha_header = tar::Header::new_gnu();
        alpha_header.set_size(alpha.len() as u64);
        alpha_header.set_mode(0o644);
        alpha_header.set_mtime(1_780_000_000);
        alpha_header.set_cksum();
        builder
            .append_data(&mut alpha_header, "alpha.txt", &mut alpha)
            .unwrap();
        builder.into_inner().unwrap()
    };
    let (tar_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": tar_uri,
            "mime": "application/x-tar",
            "data": base64::engine::general_purpose::STANDARD.encode(tar_bytes),
        }),
    )
    .await;
    assert_eq!(tar_write_status, StatusCode::OK);
    let (tar_entries_status, tar_entries) = post_library(
        app,
        &token,
        "archive_entries",
        json!({
            "uri": tar_uri,
        }),
    )
    .await;
    assert_eq!(tar_entries_status, StatusCode::OK);
    assert_eq!(tar_entries["data"]["family"], "tar");
    assert!(tar_entries["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["path"] == "alpha.txt"
                && entry["size"] == 9
                && entry["modified_at"] == 1_780_000_000u64
                && entry["safety"]["status"] == "safe"
        }));
}

#[tokio::test]
async fn test_library_provider_lists_unsafe_archive_entries_as_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let zip_uri = format!("{documents_uri}/Unsafe.zip");
    let tar_uri = format!("{documents_uri}/Unsafe.tar");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let zip_bytes = {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"escape").unwrap();
        writer.start_file("/absolute.txt", options).unwrap();
        writer.write_all(b"absolute").unwrap();
        writer.start_file("safe.txt", options).unwrap();
        writer.write_all(b"safe").unwrap();
        writer.finish().unwrap().into_inner()
    };
    let (zip_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": zip_uri,
            "mime": "application/zip",
            "data": base64::engine::general_purpose::STANDARD.encode(zip_bytes),
        }),
    )
    .await;
    assert_eq!(zip_write_status, StatusCode::OK);
    let (zip_entries_status, zip_entries) = post_library(
        app.clone(),
        &token,
        "archive_entries",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(zip_entries_status, StatusCode::OK);
    assert_eq!(zip_entries["status"], "ok");
    let zip_rows = zip_entries["data"]["entries"].as_array().unwrap();
    assert!(zip_rows.iter().any(|entry| {
        entry["path"] == "../escape.txt"
            && entry["kind"] == "blocked"
            && entry["safety"]["status"] == "blocked"
            && entry["safety"]["reason"]
                .as_str()
                .unwrap()
                .contains("relative and safe")
    }));
    assert!(zip_rows
        .iter()
        .any(|entry| entry["path"] == "safe.txt" && entry["safety"]["status"] == "safe"));
    assert!(zip_rows.iter().any(|entry| {
        entry["path"] == "/absolute.txt"
            && entry["kind"] == "blocked"
            && entry["safety"]["reason"]
                .as_str()
                .unwrap()
                .contains("relative and safe")
    }));

    let tar_bytes = {
        let mut builder = tar::Builder::new(Vec::new());
        let mut alpha = b"tar safe".as_slice();
        let mut alpha_header = tar::Header::new_gnu();
        alpha_header.set_size(alpha.len() as u64);
        alpha_header.set_mode(0o644);
        alpha_header.set_cksum();
        builder
            .append_data(&mut alpha_header, "safe.txt", &mut alpha)
            .unwrap();
        let mut link_header = tar::Header::new_gnu();
        link_header.set_entry_type(tar::EntryType::Symlink);
        link_header.set_size(0);
        link_header.set_mode(0o644);
        link_header.set_cksum();
        builder
            .append_link(&mut link_header, "link.txt", "safe.txt")
            .unwrap();
        builder.into_inner().unwrap()
    };
    let (tar_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": tar_uri,
            "mime": "application/x-tar",
            "data": base64::engine::general_purpose::STANDARD.encode(tar_bytes),
        }),
    )
    .await;
    assert_eq!(tar_write_status, StatusCode::OK);
    let (tar_entries_status, tar_entries) = post_library(
        app,
        &token,
        "archive_entries",
        json!({
            "uri": tar_uri,
        }),
    )
    .await;
    assert_eq!(tar_entries_status, StatusCode::OK);
    let tar_rows = tar_entries["data"]["entries"].as_array().unwrap();
    assert!(tar_rows
        .iter()
        .any(|entry| entry["path"] == "safe.txt" && entry["safety"]["status"] == "safe"));
    assert!(tar_rows.iter().any(|entry| {
        entry["path"] == "link.txt"
            && entry["kind"] == "blocked"
            && entry["safety"]["reason"]
                .as_str()
                .unwrap()
                .contains("non-file")
    }));
}

#[tokio::test]
async fn test_library_provider_selectively_extracts_archive_entries_through_viewer_route() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let archive_token = app_token_for_authority(dir.path(), "archive-manager", &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let imports_uri = format!("{documents_uri}/Imports");
    let nested_imports_uri = format!("{imports_uri}/Nested");
    let zip_uri = format!("{documents_uri}/Bundle.zip");
    let encoded_zip_uri = zip_uri.replace(':', "%3A").replace('/', "%2F");
    write_test_static_capsule(
        dir.path(),
        "archive-manager",
        "viewer",
        "Test Archive viewer",
        "<!doctype html><title>Archive</title>",
    );

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (imports_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Imports",
        }),
    )
    .await;
    assert_eq!(imports_status, StatusCode::OK);
    let (nested_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": imports_uri,
            "name": "Nested",
        }),
    )
    .await;
    assert_eq!(nested_status, StatusCode::OK);

    let zip_bytes = {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("alpha.txt", options).unwrap();
        writer.write_all(b"zip alpha").unwrap();
        writer.start_file("Nested/deep.txt", options).unwrap();
        writer.write_all(b"zip nested").unwrap();
        writer.finish().unwrap().into_inner()
    };
    let (zip_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": zip_uri,
            "mime": "application/zip",
            "data": base64::engine::general_purpose::STANDARD.encode(zip_bytes),
        }),
    )
    .await;
    assert_eq!(zip_write_status, StatusCode::OK);
    let (existing_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": format!("{nested_imports_uri}/deep.txt"),
            "mime": "text/plain",
            "data": base64::engine::general_purpose::STANDARD.encode(b"existing"),
        }),
    )
    .await;
    assert_eq!(existing_status, StatusCode::OK);

    let direct_extract = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/archive_extract_entries")
                .header("x-elastos-home-token", archive_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "uri": zip_uri,
                        "destination_uri": imports_uri,
                        "entries": ["Nested/deep.txt"],
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_extract.status(), StatusCode::FORBIDDEN);

    let viewer_extract = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_zip_uri}"
                ))
                .header("x-elastos-home-token", archive_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "destination_uri": imports_uri,
                        "entries": ["Nested/deep.txt"],
                        "conflict_policy": "replace",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_status = viewer_extract.status();
    let viewer_body = axum::body::to_bytes(viewer_extract.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_body)
    );
    let viewer_extract: serde_json::Value = serde_json::from_slice(&viewer_body).unwrap();
    assert_eq!(viewer_extract["status"], "ok");
    assert_eq!(
        viewer_extract["data"]["schema"],
        "elastos.library.archive-extract-entries/v1"
    );
    assert_eq!(viewer_extract["data"]["receipt"]["status"], "completed");
    assert_eq!(
        viewer_extract["data"]["receipt"]["progress"]["requested_entries"],
        1
    );
    assert_eq!(
        viewer_extract["data"]["receipt"]["progress"]["written_entries"],
        1
    );
    assert_eq!(
        viewer_extract["data"]["receipt"]["cancel"]["status"],
        "not_requested"
    );

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": format!("{nested_imports_uri}/deep.txt"),
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "zip nested");

    let (cancel_status, cancel) = post_library(
        app.clone(),
        &token,
        "archive_extract_entries",
        json!({
            "uri": zip_uri,
            "destination_uri": imports_uri,
            "entries": ["alpha.txt"],
            "cancel": true,
        }),
    )
    .await;
    assert_eq!(cancel_status, StatusCode::OK);
    assert_eq!(cancel["status"], "ok");
    assert_eq!(cancel["data"]["receipt"]["status"], "cancelled");
    assert_eq!(
        cancel["data"]["receipt"]["cancel"]["status"],
        "cancelled_before_write"
    );

    let (list_status, list) = post_library(
        app,
        &token,
        "list",
        json!({
            "uri": imports_uri,
        }),
    )
    .await;
    assert_eq!(list_status, StatusCode::OK);
    assert!(!list["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| object["name"] == "alpha.txt"));
}

#[tokio::test]
async fn test_library_provider_selective_extract_blocks_unsafe_entries() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let imports_uri = format!("{documents_uri}/Imports");
    let tar_uri = format!("{documents_uri}/Unsafe.tar");
    let zip_uri = format!("{documents_uri}/Unsafe.zip");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (imports_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Imports",
        }),
    )
    .await;
    assert_eq!(imports_status, StatusCode::OK);

    let tar_bytes = {
        let mut builder = tar::Builder::new(Vec::new());
        let mut link_header = tar::Header::new_gnu();
        link_header.set_entry_type(tar::EntryType::Symlink);
        link_header.set_size(0);
        link_header.set_mode(0o644);
        link_header.set_cksum();
        builder
            .append_link(&mut link_header, "link.txt", "safe.txt")
            .unwrap();
        builder.into_inner().unwrap()
    };
    let (tar_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": tar_uri,
            "mime": "application/x-tar",
            "data": base64::engine::general_purpose::STANDARD.encode(tar_bytes),
        }),
    )
    .await;
    assert_eq!(tar_write_status, StatusCode::OK);
    let (extract_status, extract) = post_library(
        app.clone(),
        &token,
        "archive_extract_entries",
        json!({
            "uri": tar_uri,
            "destination_uri": imports_uri,
            "entries": ["link.txt"],
        }),
    )
    .await;
    assert_eq!(extract_status, StatusCode::OK);
    assert_eq!(extract["status"], "ok");
    assert_eq!(
        extract["data"]["receipt"]["status"],
        "completed_with_blocked_entries"
    );
    assert_eq!(extract["data"]["receipt"]["progress"]["blocked_entries"], 1);
    assert!(extract["data"]["blocked"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("non-file"));

    let zip_bytes = {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"escape").unwrap();
        writer.finish().unwrap().into_inner()
    };
    let (zip_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": zip_uri,
            "mime": "application/zip",
            "data": base64::engine::general_purpose::STANDARD.encode(zip_bytes),
        }),
    )
    .await;
    assert_eq!(zip_write_status, StatusCode::OK);
    let (unsafe_select_status, unsafe_select) = post_library(
        app,
        &token,
        "archive_extract_entries",
        json!({
            "uri": zip_uri,
            "destination_uri": imports_uri,
            "entries": ["../escape.txt"],
        }),
    )
    .await;
    assert_eq!(unsafe_select_status, StatusCode::OK);
    assert_eq!(unsafe_select["status"], "error");
    assert!(unsafe_select["message"]
        .as_str()
        .unwrap()
        .contains("relative and safe"));
}

#[tokio::test]
async fn test_library_provider_rejects_unsafe_zip_entries() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let zip_uri = format!("{documents_uri}/Unsafe.zip");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let archive_bytes = {
        use std::io::Write as _;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"escape").unwrap();
        writer.finish().unwrap().into_inner()
    };

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": zip_uri,
            "mime": "application/zip",
            "data": base64::engine::general_purpose::STANDARD.encode(archive_bytes),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (extract_status, extracted) = post_library(
        app,
        &token,
        "extract_archive",
        json!({
            "uri": zip_uri,
        }),
    )
    .await;
    assert_eq!(extract_status, StatusCode::OK);
    assert_eq!(extracted["status"], "error");
    assert!(extracted["message"]
        .as_str()
        .unwrap()
        .contains("relative and safe"));
}

#[tokio::test]
async fn test_library_gateway_reads_webspace_files_through_runtime_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let uri = format!("localhost://WebSpaces/Elastos/content/{TEST_CIDV1}");
    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(read["status"], "ok");
    assert_eq!(read["data"]["object"]["kind"], "file");
    assert_eq!(
        read["data"]["object"]["metadata"]["target_uri"],
        format!("elastos://{TEST_CIDV1}")
    );
    assert_eq!(
        read["data"]["object"]["metadata"]["provider"],
        "content-provider"
    );
    assert_eq!(
        read["data"]["object"]["metadata"]["webspace_kind"],
        "file-endpoint"
    );
    assert_eq!(read["data"]["object"]["metadata"]["resolver"], "builtin");
    assert_eq!(read["data"]["encoding"], "base64");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["target_uri"], format!("elastos://{TEST_CIDV1}"));

    let (download_status, headers, download_bytes) = get_library_download(app, &token, &uri).await;
    assert_eq!(download_status, StatusCode::OK);
    assert_eq!(download_bytes, bytes);
    assert_eq!(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json"),
    );
    let expected_disposition = format!("attachment; filename=\"{TEST_CIDV1}\"");
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some(expected_disposition.as_str()),
    );
}

#[tokio::test]
async fn test_library_gateway_reads_external_webspace_file_through_adapter_cache() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let uri = "localhost://WebSpaces/Cloud/Drive/Project X/file.pdf";
    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(read["status"], "ok");
    assert_eq!(read["data"]["object"]["kind"], "file");
    assert_eq!(
        read["data"]["object"]["metadata"]["resolver"],
        "cloud-drive"
    );
    assert_eq!(
        read["data"]["object"]["metadata"]["target_uri"],
        "cloud://drive/Drive/Project X/file.pdf"
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, b"cloud adapter bytes");
}

#[tokio::test]
async fn test_library_gateway_operator_webspace_adapter_caches_bytes_and_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    write_test_static_capsule(
        dir.path(),
        DOCUMENTS_CAPSULE_ID,
        "viewer",
        "Test Documents viewer",
        "<!doctype html><title>Documents Viewer</title>",
    );

    let uri = "localhost://WebSpaces/Operator/Projects/Brief.md";
    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(read["status"], "ok");
    let object = &read["data"]["object"];
    assert_eq!(object["kind"], "file");
    assert_eq!(object["mime"], "text/plain");
    assert_eq!(object["viewer"], "documents");
    assert_eq!(object["viewers"][0]["id"], "documents");
    assert_eq!(object["metadata"]["resolver"], "operator-drive");
    assert_eq!(
        object["metadata"]["target_uri"],
        "operator://drive/Projects/Brief.md"
    );
    assert_eq!(object["metadata"]["cache_state"], "content_cached");
    assert_eq!(object["metadata"]["resolver_state"], "materialized-local");
    assert_eq!(object["metadata"]["webspace_kind"], "materialized-file");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, b"# Operator Brief\n\nAdapter-backed bytes.\n");

    let (stat_status, stat) = post_library(
        app.clone(),
        &token,
        "stat",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(stat_status, StatusCode::OK);
    assert_eq!(stat["status"], "ok");
    assert_eq!(
        stat["data"]["object"]["metadata"]["cache_state"],
        "content_cached"
    );
    assert_eq!(stat["data"]["object"]["viewer"], "documents");

    let (second_read_status, second_read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(second_read_status, StatusCode::OK);
    let second_bytes = base64::engine::general_purpose::STANDARD
        .decode(second_read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(second_bytes, bytes);
    assert_eq!(
        second_read["data"]["object"]["metadata"]["cache_state"],
        "content_cached"
    );
}

#[tokio::test]
async fn test_library_gateway_lists_external_webspace_archive_entries_without_resolver_leak() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let archive_token = app_token_for_authority(dir.path(), "archive-manager", &authority);
    write_test_static_capsule(
        dir.path(),
        "archive-manager",
        "viewer",
        "Test Archive viewer",
        "<!doctype html><title>Archive</title>",
    );

    let uri = "localhost://WebSpaces/Operator/Projects/Bundle.zip";
    let encoded_uri = uri.replace(':', "%3A").replace('/', "%2F");
    let viewer_entries = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_uri}&entries=true"
                ))
                .header("x-elastos-home-token", archive_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_entries_status = viewer_entries.status();
    let viewer_body = axum::body::to_bytes(viewer_entries.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_entries_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_body)
    );
    let viewer_entries: serde_json::Value = serde_json::from_slice(&viewer_body).unwrap();
    assert_eq!(viewer_entries["status"], "ok");
    assert_eq!(
        viewer_entries["data"]["schema"],
        "elastos.library.archive-entries/v1"
    );
    assert_eq!(viewer_entries["data"]["family"], "zip");
    assert_eq!(
        viewer_entries["data"]["object"]["metadata"]["resolver_target_redacted"],
        true
    );
    assert_eq!(
        viewer_entries["data"]["object"]["metadata"]["target_uri"],
        serde_json::Value::Null
    );
    assert!(viewer_entries["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "Nested/deep.txt" && entry["safety"]["status"] == "safe"));
    assert!(!serde_json::to_string(&viewer_entries)
        .unwrap()
        .contains("operator://"));

    let viewer_preview = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_uri}&preview_entry=Nested%2Fdeep.txt"
                ))
                .header("x-elastos-home-token", archive_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_preview_status = viewer_preview.status();
    let viewer_preview_body = axum::body::to_bytes(viewer_preview.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_preview_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_preview_body)
    );
    let viewer_preview: serde_json::Value = serde_json::from_slice(&viewer_preview_body).unwrap();
    assert_eq!(
        viewer_preview["data"]["schema"],
        "elastos.library.archive-preview-entry/v1"
    );
    assert_eq!(viewer_preview["data"]["preview"]["text"], "zip nested");
    assert!(!serde_json::to_string(&viewer_preview)
        .unwrap()
        .contains("operator://"));

    let (provider_status, provider_entries) = post_library(
        app,
        &token,
        "archive_entries",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(provider_status, StatusCode::OK);
    assert_eq!(provider_entries["status"], "ok");
    assert!(!serde_json::to_string(&provider_entries)
        .unwrap()
        .contains("operator://"));
}

#[tokio::test]
async fn test_library_gateway_imports_external_webspace_archive_entries_to_local_library() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let archive_token = app_token_for_authority(dir.path(), "archive-manager", &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let imports_uri = format!("{documents_uri}/Imports");
    let source_uri = "localhost://WebSpaces/Operator/Projects/Bundle.zip";
    let encoded_source_uri = source_uri.replace(':', "%3A").replace('/', "%2F");
    write_test_static_capsule(
        dir.path(),
        "archive-manager",
        "viewer",
        "Test Archive viewer",
        "<!doctype html><title>Archive</title>",
    );

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Documents",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    let (imports_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Imports",
        }),
    )
    .await;
    assert_eq!(imports_status, StatusCode::OK);

    let viewer_extract = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/viewers/archive-manager/library-object?uri={encoded_source_uri}"
                ))
                .header("x-elastos-home-token", archive_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "destination_uri": imports_uri,
                        "entries": ["Nested/deep.txt"],
                        "conflict_policy": "replace",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let viewer_extract_status = viewer_extract.status();
    let viewer_body = axum::body::to_bytes(viewer_extract.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        viewer_extract_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&viewer_body)
    );
    let viewer_extract: serde_json::Value = serde_json::from_slice(&viewer_body).unwrap();
    assert_eq!(viewer_extract["status"], "ok");
    assert_eq!(
        viewer_extract["data"]["schema"],
        "elastos.library.archive-extract-entries/v1"
    );
    assert_eq!(viewer_extract["data"]["receipt"]["status"], "completed");
    assert_eq!(
        viewer_extract["data"]["written"][0]["uri"],
        format!("{imports_uri}/Nested/deep.txt")
    );
    assert!(!serde_json::to_string(&viewer_extract)
        .unwrap()
        .contains("operator://"));

    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": format!("{imports_uri}/Nested/deep.txt"),
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "zip nested");
}

#[tokio::test]
async fn test_library_gateway_webspace_archive_writeback_requires_mutable_write_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let source_uri = "localhost://WebSpaces/Operator/Projects/Bundle.zip";

    let (readonly_status, readonly) = post_library(
        app.clone(),
        &token,
        "archive_extract_entries",
        json!({
            "uri": source_uri,
            "destination_uri": "localhost://WebSpaces/Operator/Projects",
            "entries": ["alpha.txt"],
            "conflict_policy": "replace",
        }),
    )
    .await;
    assert_eq!(readonly_status, StatusCode::OK);
    assert_eq!(readonly["status"], "error");
    assert!(
        readonly["message"]
            .as_str()
            .unwrap()
            .contains("mutable destination Space"),
        "{readonly}"
    );

    let (writeback_status, writeback) = post_library(
        app.clone(),
        &token,
        "archive_extract_entries",
        json!({
            "uri": source_uri,
            "destination_uri": "localhost://WebSpaces/OperatorMutable/Folder",
            "entries": ["alpha.txt"],
            "conflict_policy": "replace",
        }),
    )
    .await;
    assert_eq!(writeback_status, StatusCode::OK);
    assert_eq!(writeback["status"], "ok");
    assert_eq!(writeback["data"]["receipt"]["status"], "completed");
    assert_eq!(
        writeback["data"]["written"][0]["webspace"]["write_back"],
        "resolver_synced"
    );
    assert_eq!(
        writeback["data"]["written"][0]["uri"],
        "localhost://WebSpaces/OperatorMutable/Folder/alpha.txt"
    );
    assert!(!serde_json::to_string(&writeback)
        .unwrap()
        .contains("operator://"));

    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": "localhost://WebSpaces/OperatorMutable/Folder/alpha.txt",
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), "zip alpha");
}

#[tokio::test]
async fn test_library_gateway_webspace_sync_caches_adapter_bytes_without_foreground_read() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    write_test_static_capsule(
        dir.path(),
        DOCUMENTS_CAPSULE_ID,
        "viewer",
        "Test Documents viewer",
        "<!doctype html><title>Documents Viewer</title>",
    );

    let uri = "localhost://WebSpaces/Operator/Projects/Brief.md";
    let expected = b"# Operator Brief\n\nAdapter-backed bytes.\n";
    let (sync_status, sync) = post_library(
        app.clone(),
        &token,
        "sync",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(sync_status, StatusCode::OK);
    assert_eq!(sync["status"], "ok");
    let receipt = &sync["data"]["receipt"];
    assert_eq!(receipt["schema"], "elastos.webspace.byte-sync-receipt/v1");
    assert_eq!(receipt["action"], "bytes_cached_from_adapter");
    assert_eq!(receipt["foreground_read"], false);
    assert_eq!(receipt["bytes_exposed"], false);
    assert_eq!(receipt["content_synced"], true);
    assert_eq!(receipt["bytes_cached"], expected.len());
    assert_eq!(
        receipt["availability_hint"]["schema"],
        "elastos.webspace.availability-hint/v1"
    );
    assert_eq!(receipt["availability_hint"]["status"], "resolver_cached");
    assert_eq!(
        receipt["availability_hint"]["target_uri"],
        "operator://drive/Projects/Brief.md"
    );
    assert_eq!(
        receipt["availability_hint"]["not_content_availability"],
        true
    );
    assert!(receipt.get("data").is_none());
    assert_eq!(sync["data"]["object"]["availability"], "resolver-cached");
    assert_eq!(
        sync["data"]["object"]["metadata"]["cache_state"],
        "content_cached"
    );
    assert_eq!(
        sync["data"]["object"]["metadata"]["availability_hint"]["status"],
        "resolver_cached"
    );
    assert_eq!(sync["data"]["object"]["viewer"], "documents");

    let (stat_status, stat) = post_library(
        app.clone(),
        &token,
        "stat",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(stat_status, StatusCode::OK);
    assert_eq!(
        stat["data"]["object"]["metadata"]["cache_state"],
        "content_cached"
    );
    assert_eq!(
        stat["data"]["object"]["metadata"]["webspace_kind"],
        "materialized-file"
    );
    assert_eq!(stat["data"]["object"]["availability"], "resolver-cached");
    assert_eq!(
        stat["data"]["object"]["metadata"]["availability_hint"]["scope"],
        "resolver"
    );

    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, expected);
    assert_eq!(
        read["data"]["object"]["metadata"]["cache_state"],
        "content_cached"
    );
    assert_eq!(read["data"]["object"]["availability"], "resolver-cached");
}

#[tokio::test]
async fn test_library_gateway_syncs_operator_mutable_webspace_file_to_resolver() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let uri = "localhost://WebSpaces/OperatorMutable/Folder/note.txt";

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"operator mutable bytes"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");
    assert_eq!(
        write["data"]["object"]["metadata"]["sync_state"],
        "manual_pending"
    );
    assert_eq!(
        write["data"]["object"]["metadata"]["target_uri"],
        "operator://drive/Writable/Folder/note.txt"
    );

    let (sync_status, sync) = post_library(
        app.clone(),
        &token,
        "sync",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(sync_status, StatusCode::OK);
    assert_eq!(sync["status"], "ok");
    let receipt = &sync["data"]["receipt"];
    assert_eq!(
        receipt["schema"],
        "elastos.webspace.resolver-sync-receipt/v1"
    );
    assert_eq!(receipt["action"], "resolver_write_synced");
    assert_eq!(receipt["resolver_synced"], true);
    assert_eq!(receipt["content_synced"], true);
    assert_eq!(receipt["fail_closed"], false);
    assert_eq!(receipt["conflict"], false);
    assert_eq!(receipt["bytes_exposed"], false);
    assert_eq!(receipt["bytes_synced"], b"operator mutable bytes".len());
    assert_eq!(receipt["provider"], "operator-drive-adapter");
    assert_eq!(
        receipt["availability_hint"]["schema"],
        "elastos.webspace.availability-hint/v1"
    );
    assert_eq!(receipt["availability_hint"]["status"], "resolver_synced");
    assert_eq!(
        receipt["availability_hint"]["not_content_availability"],
        true
    );
    assert_eq!(
        receipt["target_uri"],
        "operator://drive/Writable/Folder/note.txt"
    );
    assert_eq!(
        receipt["adapter_receipt"]["schema"],
        "elastos.webspace.adapter.write-bytes-receipt/v1"
    );
    assert_eq!(
        sync["data"]["object"]["metadata"]["sync_state"],
        "manual_synced"
    );
    assert_eq!(sync["data"]["object"]["availability"], "resolver-synced");
    assert_eq!(
        sync["data"]["object"]["metadata"]["availability_hint"]["status"],
        "resolver_synced"
    );

    let (stat_status, stat) = post_library(
        app,
        &token,
        "stat",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(stat_status, StatusCode::OK);
    assert_eq!(
        stat["data"]["object"]["metadata"]["sync_state"],
        "manual_synced"
    );
    assert_eq!(stat["data"]["object"]["availability"], "resolver-synced");
}

#[tokio::test]
async fn test_library_gateway_webspace_sync_fails_closed_without_write_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let uri = "localhost://WebSpaces/Mutable/Folder/no-adapter.txt";

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"local mutable bytes"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");
    assert_eq!(
        write["data"]["object"]["metadata"]["sync_state"],
        "manual_pending"
    );

    let (sync_status, sync) = post_library(
        app,
        &token,
        "sync",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(sync_status, StatusCode::OK);
    assert_eq!(sync["status"], "ok");
    let receipt = &sync["data"]["receipt"];
    assert_eq!(
        receipt["schema"],
        "elastos.webspace.resolver-sync-receipt/v1"
    );
    assert_eq!(receipt["action"], "resolver_write_unavailable");
    assert_eq!(receipt["resolver_synced"], false);
    assert_eq!(receipt["content_synced"], false);
    assert_eq!(receipt["fail_closed"], true);
    assert_eq!(receipt["conflict"], false);
    assert_eq!(receipt["bytes_exposed"], false);
    assert_eq!(
        sync["data"]["object"]["metadata"]["sync_state"],
        "manual_pending"
    );
}

#[tokio::test]
async fn test_library_gateway_webspace_sync_reports_resolver_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let uri = "localhost://WebSpaces/OperatorMutable/Conflict/stale.txt";

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"stale fork bytes"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (sync_status, sync) = post_library(
        app,
        &token,
        "sync",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(sync_status, StatusCode::OK);
    assert_eq!(sync["status"], "ok");
    let receipt = &sync["data"]["receipt"];
    assert_eq!(
        receipt["schema"],
        "elastos.webspace.resolver-sync-receipt/v1"
    );
    assert_eq!(receipt["action"], "resolver_write_conflict");
    assert_eq!(receipt["resolver_synced"], false);
    assert_eq!(receipt["content_synced"], false);
    assert_eq!(receipt["fail_closed"], true);
    assert_eq!(receipt["conflict"], true);
    assert_eq!(receipt["provider"], "operator-drive-adapter");
    assert_eq!(
        receipt["adapter_response"]["data"]["schema"],
        "elastos.webspace.adapter.write-conflict/v1"
    );
    assert_eq!(
        sync["data"]["object"]["metadata"]["sync_state"],
        "manual_pending"
    );
}

#[tokio::test]
async fn test_library_gateway_rejects_webspace_mutation_as_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let (write_status, write) = post_library(
        app,
        &token,
        "write",
        json!({
            "uri": "localhost://WebSpaces/Elastos/not-allowed.txt",
            "data": base64::engine::general_purpose::STANDARD.encode(b"must fail"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "error");
    assert!(write["message"]
        .as_str()
        .unwrap()
        .contains("resolver-owned and read-only"));
}

#[tokio::test]
async fn test_library_gateway_mutates_writable_webspace_through_runtime_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_webspace_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let (list_status, list) = post_library(
        app.clone(),
        &token,
        "list",
        json!({
            "uri": "localhost://WebSpaces/Mutable",
        }),
    )
    .await;
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(list["status"], "ok");
    assert_eq!(list["data"]["object"]["metadata"]["readonly"], false);
    assert_eq!(
        list["data"]["object"]["metadata"]["access_policy"],
        "owner-writable"
    );

    let (mkdir_status, mkdir) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": "localhost://WebSpaces/Mutable",
            "name": "Folder",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    assert_eq!(mkdir["status"], "ok");
    assert_eq!(
        mkdir["data"]["receipt"]["schema"],
        "elastos.webspace.mkdir-receipt/v1"
    );
    assert_eq!(mkdir["data"]["object"]["metadata"]["readonly"], false);

    let note_uri = "localhost://WebSpaces/Mutable/Folder/note.txt";
    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": note_uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"mutable bytes"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");
    assert_eq!(
        write["data"]["receipt"]["schema"],
        "elastos.webspace.write-receipt/v1"
    );
    assert_eq!(
        write["data"]["object"]["metadata"]["webspace_kind"],
        "materialized-file"
    );
    assert_eq!(write["data"]["object"]["metadata"]["readonly"], false);
    assert!(write["data"]["object"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "delete_permanently"));

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": note_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(read["status"], "ok");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, b"mutable bytes");

    let upload_uri = "localhost://WebSpaces/Mutable/Folder/upload.txt";
    let (upload_status, _upload_headers, upload) =
        put_library_upload(app.clone(), &token, upload_uri, b"uploaded bytes").await;
    assert_eq!(upload_status, StatusCode::OK);
    assert_eq!(upload["status"], "ok");
    assert_eq!(
        upload["data"]["receipt"]["schema"],
        "elastos.object.transfer.receipt/v1"
    );
    assert_eq!(
        upload["data"]["provider_receipt"]["schema"],
        "elastos.webspace.write-receipt/v1"
    );
    assert_eq!(
        upload["data"]["object"]["metadata"]["webspace_kind"],
        "materialized-file"
    );

    let (delete_status, deleted) = post_library(
        app,
        &token,
        "delete_permanently",
        json!({
            "uri": note_uri,
        }),
    )
    .await;
    assert_eq!(delete_status, StatusCode::OK);
    assert_eq!(deleted["status"], "ok");
    assert_eq!(
        deleted["data"]["receipt"]["schema"],
        "elastos.webspace.delete-receipt/v1"
    );
    assert_eq!(deleted["data"]["deleted_uri"], note_uri);
}

#[tokio::test]
async fn test_library_provider_move_is_principal_scoped_and_audited() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let target_uri = format!("{documents_uri}/Moved");
    let source_uri = format!("{documents_uri}/move-me.txt");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Moved",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": source_uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"moved bytes"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    let revision = write["data"]["object"]["revision"].as_str().unwrap();

    let (move_status, moved) = post_library(
        app.clone(),
        &token,
        "move",
        json!({
            "uri": source_uri,
            "target_parent_uri": target_uri,
            "if_revision": revision,
        }),
    )
    .await;
    assert_eq!(move_status, StatusCode::OK);
    let moved_uri = moved["data"]["object"]["uri"].as_str().unwrap();
    assert_eq!(moved_uri, format!("{target_uri}/move-me.txt"));

    let (read_status, read) = post_library(
        app.clone(),
        &token,
        "read",
        json!({
            "uri": moved_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(read["data"]["data"].as_str().unwrap())
            .unwrap(),
        b"moved bytes"
    );

    let (events_status, events) = post_library(
        app,
        &token,
        "events",
        json!({
            "uri": target_uri,
        }),
    )
    .await;
    assert_eq!(events_status, StatusCode::OK);
    assert!(events["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["op"] == "move" && event["details"]["old_uri"] == source_uri));
}

#[tokio::test]
async fn test_library_provider_copy_preserves_source_and_audits() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let target_uri = format!("{documents_uri}/Copied");
    let source_uri = format!("{documents_uri}/copy-me.txt");

    let (mkdir_status, _) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": documents_uri,
            "name": "Copied",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": source_uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"copied bytes"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    let revision = write["data"]["object"]["revision"].as_str().unwrap();

    let (copy_status, copied) = post_library(
        app.clone(),
        &token,
        "copy",
        json!({
            "uri": source_uri,
            "target_parent_uri": target_uri,
            "if_revision": revision,
        }),
    )
    .await;
    assert_eq!(copy_status, StatusCode::OK);
    let copied_uri = copied["data"]["object"]["uri"].as_str().unwrap();
    assert_eq!(copied_uri, format!("{target_uri}/copy-me.txt"));

    for uri in [&source_uri, copied_uri] {
        let (read_status, read) = post_library(
            app.clone(),
            &token,
            "read",
            json!({
                "uri": uri,
            }),
        )
        .await;
        assert_eq!(read_status, StatusCode::OK);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(read["data"]["data"].as_str().unwrap())
                .unwrap(),
            b"copied bytes"
        );
    }

    let (events_status, events) = post_library(
        app,
        &token,
        "events",
        json!({
            "uri": target_uri,
        }),
    )
    .await;
    assert_eq!(events_status, StatusCode::OK);
    assert!(events["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["op"] == "copy" && event["details"]["source_uri"] == source_uri));
}

#[tokio::test]
async fn test_library_provider_events_returns_typed_object_events() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let notes_uri = format!("{root}/Documents/events.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": notes_uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"evented"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (mkdir_status, mkdir) = post_library(
        app.clone(),
        &token,
        "mkdir",
        json!({
            "parent_uri": root,
            "name": "Event Folder",
        }),
    )
    .await;
    assert_eq!(mkdir_status, StatusCode::OK);
    assert_eq!(mkdir["status"], "ok");

    let (events_status, events) = post_library(app.clone(), &token, "events", json!({})).await;
    assert_eq!(events_status, StatusCode::OK);
    assert_eq!(events["status"], "ok");
    assert_eq!(events["data"]["schema"], "elastos.library.events/v1");
    let events = events["data"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| {
        event["schema"] == "elastos.library.event/v1"
            && event["event_id"]
                .as_str()
                .unwrap()
                .starts_with("library:event:")
    }));
    assert_eq!(events[0]["op"], "write");
    assert_eq!(events[0]["uri"], notes_uri);
    assert_eq!(events[0]["details"]["object"]["name"], "events.txt");
    assert_eq!(events[1]["op"], "mkdir");

    let (filtered_status, filtered) = post_library(
        app.clone(),
        &token,
        "events",
        json!({
            "uri": notes_uri,
        }),
    )
    .await;
    assert_eq!(filtered_status, StatusCode::OK);
    let filtered_events = filtered["data"]["events"].as_array().unwrap();
    assert_eq!(filtered_events.len(), 1);
    assert_eq!(filtered_events[0]["op"], "write");

    let (limited_status, limited) = post_library(
        app,
        &token,
        "events",
        json!({
            "limit": 1,
        }),
    )
    .await;
    assert_eq!(limited_status, StatusCode::OK);
    let limited_events = limited["data"]["events"].as_array().unwrap();
    assert_eq!(limited_events.len(), 1);
    assert_eq!(limited_events[0]["op"], "mkdir");
}

#[tokio::test]
async fn test_library_provider_events_stream_requires_library_token_and_serves_sse() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let unauthorized = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/provider/object/events/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);

    let query_authority = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(format!(
                    "/api/provider/object/events/stream?home_token={token}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(query_authority.status(), StatusCode::FORBIDDEN);

    let authorized = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/provider/object/events/stream")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
    assert!(
        authorized
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "Library event stream should be served as SSE"
    );
    assert_eq!(
        authorized
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache, no-transform"),
        "Library SSE must not be cached or transformed by proxies"
    );
    assert_eq!(
        authorized
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "nginx must not buffer realtime Library events"
    );
}

#[tokio::test]
async fn test_library_provider_viewers_only_include_installed_viewer_capsules() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/view-me.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"viewer routing"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (without_viewer_status, without_viewer) = post_library(
        app.clone(),
        &token,
        "stat",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(without_viewer_status, StatusCode::OK);
    assert!(without_viewer["data"]["object"]["viewer"].is_null());
    assert!(without_viewer["data"]["object"]["viewers"].is_null());

    write_test_static_capsule(
        dir.path(),
        DOCUMENTS_CAPSULE_ID,
        "viewer",
        "Test Documents viewer",
        "<!doctype html><title>Documents Viewer</title>",
    );

    let (with_viewer_status, with_viewer) = post_library(
        app.clone(),
        &token,
        "stat",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(with_viewer_status, StatusCode::OK);
    assert_eq!(
        with_viewer["data"]["object"]["viewer"],
        DOCUMENTS_CAPSULE_ID
    );
    assert_eq!(
        with_viewer["data"]["object"]["viewers"][0]["id"],
        DOCUMENTS_CAPSULE_ID
    );
    assert_eq!(
        with_viewer["data"]["object"]["viewers"][0]["label"],
        "Documents"
    );

    let rom_uri = format!("{root}/Documents/game.gba");
    let (rom_write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": rom_uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"rom bytes"),
        }),
    )
    .await;
    assert_eq!(rom_write_status, StatusCode::OK);

    write_test_static_capsule(
        dir.path(),
        GBA_EMULATOR_CAPSULE_ID,
        "viewer",
        "Test GBA emulator",
        "<!doctype html><title>GBA Emulator</title>",
    );

    let (rom_viewer_status, rom_viewer) = post_library(
        app,
        &token,
        "stat",
        json!({
            "uri": rom_uri,
        }),
    )
    .await;
    assert_eq!(rom_viewer_status, StatusCode::OK);
    assert_eq!(
        rom_viewer["data"]["object"]["viewer"],
        GBA_EMULATOR_CAPSULE_ID
    );
    assert_eq!(
        rom_viewer["data"]["object"]["viewers"][0]["label"],
        "GBA Emulator"
    );
}

#[tokio::test]
async fn test_library_provider_rejects_traversal_segments() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);

    let (status, payload) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": format!("{root}/Documents/../Secrets.txt"),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    assert!(payload["message"].as_str().unwrap().contains("traversal"));
}

#[tokio::test]
async fn test_library_provider_scopes_to_launch_principal() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &admin);
    let guest_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &guest);
    let admin_root = crate::auth::principal_localhost_root(&admin.principal_id);
    let guest_root = crate::auth::principal_localhost_root(&guest.principal_id);
    let admin_uri = format!("{admin_root}/Documents/private.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &admin_token,
        "write",
        json!({
            "uri": admin_uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"admin only"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (guest_status, guest_read) = post_library(
        app,
        &guest_token,
        "read",
        json!({
            "uri": admin_uri,
        }),
    )
    .await;
    assert_eq!(guest_status, StatusCode::OK);
    assert_eq!(guest_read["status"], "error");
    assert!(guest_read["message"]
        .as_str()
        .unwrap()
        .contains("outside the active principal root"));
    assert_ne!(admin_root, guest_root);
}

#[tokio::test]
async fn test_library_provider_audits_provider_operations() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);

    let (status, payload) = post_library(app, &token, "roots", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "ok");

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let library_events: Vec<_> = auth_state
        .audit
        .iter()
        .filter(|event| event.event_type.starts_with("object.provider."))
        .collect();
    assert_eq!(library_events.len(), 2);
    assert_eq!(library_events[0].event_type, "object.provider.requested");
    assert_eq!(library_events[0].result, "requested");
    assert_eq!(library_events[1].event_type, "object.provider.completed");
    assert_eq!(library_events[1].result, "completed");
    assert_eq!(
        library_events[0].challenge_id,
        library_events[1].challenge_id
    );
    assert_eq!(
        library_events[0].capsule_id.as_deref(),
        Some(LIBRARY_CAPSULE_ID)
    );
    assert!(library_events[0].reason.contains("roots"));
}

#[tokio::test]
async fn test_library_provider_writes_protected_principal_objects() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/protected.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"encrypted object"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let raw_path = rooted_localhost_fs_path(dir.path(), &uri).unwrap();
    let raw = std::fs::read_to_string(raw_path).unwrap();
    assert!(!raw.contains("encrypted object"));
    assert!(raw.contains("elastos.principal-root.object/v1"));

    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, b"encrypted object");
}

#[tokio::test]
async fn test_library_provider_auto_protects_plaintext_legacy_objects() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let documents_uri = format!("{root}/Documents");
    let secret_uri = format!("{documents_uri}/secret.md");
    let secret_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &secret_uri).unwrap();
    std::fs::create_dir_all(secret_path.parent().unwrap()).unwrap();
    std::fs::write(&secret_path, b"plaintext from an older runtime").unwrap();

    let (list_status, list) = post_library(
        app.clone(),
        &token,
        "list",
        json!({
            "uri": documents_uri,
        }),
    )
    .await;
    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(list["status"], "ok");
    let object = list["data"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|object| object["uri"] == secret_uri)
        .expect("legacy object should still appear in folder listing");
    assert!(object["blocked_reason"].is_null());
    assert_eq!(object["availability"], "local-only");
    assert!(object["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "read"));
    assert_ne!(
        std::fs::read(&secret_path).unwrap(),
        b"plaintext from an older runtime",
        "listing should migrate protected-root plaintext to encrypted storage",
    );

    let (read_status, read) = post_library(
        app,
        &token,
        "read",
        json!({
            "uri": secret_uri,
        }),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert_eq!(read["status"], "ok");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(read["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, b"plaintext from an older runtime");
}

#[tokio::test]
async fn test_library_provider_publish_fails_closed_without_content_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/no-content.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"publish me"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (publish_status, publish) = post_library(
        app,
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);
    assert_eq!(publish["status"], "error");
    assert!(publish["message"]
        .as_str()
        .unwrap()
        .contains("content provider unavailable"));
}

#[tokio::test]
async fn test_library_provider_publish_uses_content_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/publish.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"publish me"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    let local_cid = write["data"]["object"]["content_cid"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(local_cid.starts_with("bafkrei"));
    assert_eq!(write["data"]["object"].get("published_cid"), None);
    assert_eq!(write["data"]["object"]["published"], false);

    let (publish_status, publish) = post_library(
        app.clone(),
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);
    assert_eq!(publish["status"], "ok");
    assert_eq!(publish["data"]["cid"], TEST_CIDV1);
    assert_eq!(publish["data"]["uri"], format!("elastos://{TEST_CIDV1}"));
    assert_eq!(publish["data"]["object"]["content_cid"], local_cid);
    assert_eq!(publish["data"]["object"]["published_cid"], TEST_CIDV1);

    let (status_code, status) = post_library(
        app,
        &token,
        "status",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(status["data"]["object"]["published"], true);
    assert_eq!(status["data"]["object"]["content_cid"], local_cid);
    assert_eq!(status["data"]["object"]["published_cid"], TEST_CIDV1);
    assert_eq!(status["data"]["published"]["cid"], TEST_CIDV1);
}

#[tokio::test]
async fn test_library_gateway_coordinates_content_for_external_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_external_provider_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/external-publish.txt");

    let (write_status, write) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"publish me externally"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);
    assert_eq!(write["status"], "ok");

    let (publish_status, publish) = post_library(
        app.clone(),
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);
    assert_eq!(publish["status"], "ok");
    assert_eq!(publish["data"]["cid"], TEST_CIDV1);

    let (status_code, status) = post_library(
        app,
        &token,
        "status",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(status["status"], "ok");
    assert_eq!(status["data"]["object"]["published"], true);
    assert_eq!(status["data"]["published"]["cid"], TEST_CIDV1);
}

#[tokio::test]
async fn test_library_provider_share_requires_active_publish_record() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/share.txt");

    let (write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"share me"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);

    let (share_draft_status, share_draft) = post_library(
        app.clone(),
        &token,
        "share",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(share_draft_status, StatusCode::OK);
    assert_eq!(share_draft["status"], "error");
    assert!(share_draft["message"]
        .as_str()
        .unwrap()
        .contains("published object"));

    let (publish_status, _) = post_library(
        app.clone(),
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);

    let (share_status, share) = post_library(
        app.clone(),
        &token,
        "share",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(share_status, StatusCode::OK);
    assert_eq!(share["status"], "ok");
    assert_eq!(share["data"]["schema"], "elastos.library.share/v1");
    assert_eq!(share["data"]["uri"], format!("elastos://{TEST_CIDV1}"));
    assert_eq!(share["data"]["policy"], "public_link");
    assert!(share["data"]["recipients"].as_array().unwrap().is_empty());
    assert!(share["data"]["grants"].as_array().unwrap().is_empty());
    assert_eq!(share["data"]["object"]["shared"], true);
}

#[tokio::test]
async fn test_library_provider_records_recipient_scoped_share_grants() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/scoped-share.txt");

    let (write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"share me to a recipient"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);

    let (publish_status, _) = post_library(
        app.clone(),
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);

    let (share_status, share) = post_library(
        app.clone(),
        &token,
        "share",
        json!({
            "uri": uri,
            "recipients": [
                authority.principal_id,
                "did:key:z6MkRecipient111111111111111111111111111111111",
                "did:key:z6MkRecipient111111111111111111111111111111111",
                "person:local:alice"
            ]
        }),
    )
    .await;
    assert_eq!(share_status, StatusCode::OK);
    assert_eq!(share["status"], "ok");
    assert_eq!(share["data"]["policy"], "recipient_scoped");
    assert_eq!(share["data"]["recipients"].as_array().unwrap().len(), 3);
    assert_eq!(share["data"]["grants"].as_array().unwrap().len(), 3);
    assert_eq!(
        share["data"]["grants"][0]["schema"],
        "elastos.library.share-grant/v1"
    );
    assert_eq!(share["data"]["grants"][0]["cid"], TEST_CIDV1);
    assert_eq!(share["data"]["grants"][0]["policy"], "recipient_scoped");
    assert_eq!(
        share["data"]["content_security"]["schema"],
        "elastos.library.published-content-security/v1"
    );
    assert_eq!(
        share["data"]["content_security"]["published_payload"],
        "plain_content"
    );
    assert_eq!(
        share["data"]["key_release"]["schema"],
        "elastos.library.key-release/v1"
    );
    assert_eq!(share["data"]["key_release"]["required"], false);
    assert_eq!(
        share["data"]["grants"][0]["key_release"]["status"],
        "not_required_for_plain_published_content"
    );
    assert_eq!(
        share["data"]["remote_enforcement"]["required_providers"]["schema"],
        "elastos.library.protected-content-provider-requirements/v1"
    );
    assert_eq!(
        share["data"]["remote_enforcement"]["provider_invocation"]["drm"],
        "drm-provider.open"
    );
    assert_eq!(
        share["data"]["remote_enforcement"]["provider_invocation"]["rights"],
        "rights-provider.has_access_by_content_id"
    );
    assert_eq!(
        share["data"]["protected_content"]["schema"],
        "elastos.library.protected-content-provider-status/v1"
    );
    assert_eq!(
        share["data"]["protected_content"]["encrypted_recipient_sharing"]["status"],
        "blocked_until_drm_rights_key_decrypt_providers_configured"
    );
    assert_eq!(
        share["data"]["protected_content"]["required_provider_count"],
        4
    );

    let (status_status, status) = post_library(
        app.clone(),
        &token,
        "status",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(status_status, StatusCode::OK);
    assert_eq!(status["status"], "ok");
    assert_eq!(
        status["data"]["protected_content"]["schema"],
        "elastos.library.protected-content-provider-status/v1"
    );
    assert_eq!(
        status["data"]["published"]["protected_content"]["encrypted_recipient_sharing"]["status"],
        "blocked_until_drm_rights_key_decrypt_providers_configured"
    );

    let (access_status, access) = post_library(
        app.clone(),
        &token,
        "shared_access",
        json!({
            "uri": uri,
            "recipient": authority.principal_id,
        }),
    )
    .await;
    assert_eq!(access_status, StatusCode::OK);
    assert_eq!(access["status"], "ok");
    assert_eq!(access["data"]["schema"], "elastos.library.shared-access/v1");
    assert_eq!(access["data"]["uri"], format!("elastos://{TEST_CIDV1}"));
    assert_eq!(access["data"]["access"]["policy"], "recipient_scoped");
    assert_eq!(
        access["data"]["access"]["recipient"],
        authority.principal_id
    );
    assert_eq!(
        access["data"]["access"]["recipient_proof"]["schema"],
        "elastos.library.recipient-proof-state/v1"
    );
    assert_eq!(
        access["data"]["access"]["recipient_proof"]["verified"],
        true
    );
    assert_eq!(
        access["data"]["access"]["recipient_proof"]["source"],
        "runtime-launch-grant"
    );
    assert_eq!(
        access["data"]["access"]["recipient_proof"]["proof_binding_id"],
        authority.proof_binding_id
    );
    assert_eq!(access["data"]["access"]["decision"]["allowed"], true);
    assert_eq!(
        access["data"]["access"]["decision"]["schema"],
        "elastos.library.access-decision/v1"
    );
    assert_eq!(
        access["data"]["access"]["open"]["schema"],
        "elastos.library.shared-open/v1"
    );
    assert_eq!(
        access["data"]["access"]["open"]["provider"],
        "content-provider"
    );
    assert_eq!(
        access["data"]["access"]["open"]["transport"],
        "runtime-provider-fetch"
    );
    assert_eq!(
        access["data"]["access"]["open"]["status"],
        "ready_for_plain_content_fetch"
    );
    assert_eq!(
        access["data"]["access"]["open"]["key_release_required"],
        false
    );
    assert_eq!(
        access["data"]["access"]["open"]["drm_provider_required"],
        false
    );
    assert_eq!(
        access["data"]["access"]["open"]["recipient_proof_verified"],
        true
    );
    assert_eq!(
        access["data"]["access"]["open"]["rights_provider_required"],
        false
    );
    assert_eq!(
        access["data"]["access"]["open"]["key_provider_required"],
        false
    );
    assert_eq!(
        access["data"]["access"]["open"]["decrypt_provider_required"],
        false
    );
    assert_eq!(
        access["data"]["access"]["open"]["required_providers"]["providers"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        access["data"]["protected_content"]["schema"],
        "elastos.library.protected-content-provider-status/v1"
    );
    assert_eq!(
        access["data"]["access"]["key_release"]["status"],
        "not_required_for_plain_published_content"
    );
    assert_eq!(
        access["data"]["access"]["content_security"]["published_payload"],
        "plain_content"
    );

    let (missing_proof_status, missing_proof) = post_library(
        app.clone(),
        &token,
        "shared_access",
        json!({
            "uri": uri,
            "recipient": "person:local:alice",
            "recipient_proof": {
                "schema": "elastos.library.recipient-proof/v1",
                "source": "runtime-launch-grant",
                "recipient": "person:local:alice"
            }
        }),
    )
    .await;
    assert_eq!(missing_proof_status, StatusCode::OK);
    assert_eq!(missing_proof["status"], "error");
    assert!(missing_proof["message"]
        .as_str()
        .unwrap()
        .contains("requires Runtime recipient_proof"));

    let (blocked_status, blocked) = post_library(
        app.clone(),
        &token,
        "shared_access",
        json!({
            "uri": uri,
            "recipient": "person:local:bob",
        }),
    )
    .await;
    assert_eq!(blocked_status, StatusCode::OK);
    assert_eq!(blocked["status"], "error");
    assert!(blocked["message"]
        .as_str()
        .unwrap()
        .contains("not authorized"));

    let (events_status, events) = post_library(
        app.clone(),
        &token,
        "events",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(events_status, StatusCode::OK);
    assert_eq!(events["status"], "ok");
    let shared_access_events = events["data"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["op"] == "shared_access")
        .collect::<Vec<_>>();
    assert!(shared_access_events.iter().any(|event| {
        event["details"]["recipient"] == authority.principal_id
            && event["details"]["allowed"] == true
            && event["details"]["open"]["status"] == "ready_for_plain_content_fetch"
            && event["details"]["open"]["recipient_proof_verified"] == true
            && event["details"]["key_release"]["required"] == false
    }));
    assert!(shared_access_events.iter().any(|event| {
        event["details"]["recipient"] == "person:local:bob"
            && event["details"]["allowed"] == false
            && event["details"]["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("not authorized")
    }));
    assert!(shared_access_events.iter().any(|event| {
        event["details"]["recipient"] == "person:local:alice"
            && event["details"]["allowed"] == false
            && event["details"]["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("requires Runtime recipient_proof")
    }));

    let (status_code, status) = post_library(
        app,
        &token,
        "status",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(status["status"], "ok");
    assert_eq!(
        status["data"]["published"]["share_policy"],
        "recipient_scoped"
    );
    assert_eq!(
        status["data"]["published"]["share_grants"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn test_library_provider_rejects_key_release_policy_until_provider_exists() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/protected-share.txt");

    let (write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"share with key release"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);

    let (publish_status, _) = post_library(
        app.clone(),
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);

    let (share_status, share) = post_library(
        app,
        &token,
        "share",
        json!({
            "uri": uri,
            "recipients": ["person:local:alice"],
            "key_release_policy": "recipient_key_release",
        }),
    )
    .await;
    assert_eq!(share_status, StatusCode::OK);
    assert_eq!(share["status"], "error");
    assert!(share["message"]
        .as_str()
        .unwrap()
        .contains("drm/rights/key/decrypt providers"));
}

#[tokio::test]
async fn test_library_provider_publish_rejects_removed_fixture_and_unknown_protection_fields() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/rejected-protected-input");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &uri, 0x40).await;

    let rejection_cases = [
        (
            json!({
                "uri": uri,
                "protected_content_fixture": true,
            }),
            "protected_content_fixture",
        ),
        (
            json!({
                "uri": uri,
                "protection": {
                    "mode": "fixture"
                }
            }),
            "unknown variant `fixture`",
        ),
        (
            json!({
                "uri": uri,
                "protection": {
                    "mode": "runtime_custody",
                    "copies": "0x1",
                    "price": "0xde0b6b3a7640000",
                    "extra": true
                }
            }),
            "unknown field `extra`",
        ),
    ];

    for (body, expected_fragment) in rejection_cases {
        let (publish_status, publish) = post_library(app.clone(), &token, "publish", body).await;
        assert_eq!(publish_status, StatusCode::OK);
        assert_eq!(publish["status"], "error");
        let message = publish["message"].as_str().unwrap();
        assert!(message.contains(expected_fragment), "{publish}");
        assert!(!message.contains(&uri));
        assert!(!message.contains(&dir.path().display().to_string()));
        assert_no_publish_records(dir.path(), &authority.principal_id);
    }

    for (field, value, expected_message) in [
        (
            "wallet_account_id",
            "wallet-account-1",
            "unknown field `wallet_account_id`",
        ),
        ("mime_type", "", "unknown field `mime_type`"),
        ("mime_type", "video/mp4", "unknown field `mime_type`"),
        ("codecs", "", "unknown field `codecs`"),
        ("codecs", "avc1.64001f,mp4a.40.2", "unknown field `codecs`"),
    ] {
        assert_rejected_runtime_custody_protection_field(
            dir.path(),
            &app,
            &token,
            &authority.principal_id,
            &uri,
            field,
            value,
            expected_message,
        )
        .await;
    }
}

#[tokio::test]
async fn test_library_provider_runtime_custody_publish_fails_closed_without_composition_or_mint_record(
) {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let protected_content_root = dir.path().join("protected-content");
    std::fs::create_dir_all(&protected_content_root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            &protected_content_root,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider(
            "object",
            std::sync::Arc::new(crate::library::ObjectProvider::new(
                dir.path().to_path_buf(),
                std::sync::Arc::downgrade(&registry),
            )),
        )
        .await
        .unwrap();
    let _media_fixture = crate::protected_content_runtime::tests::register_runtime_custody_mock_media_provider_for_test_registry(
        dir.path(),
        &registry,
    )
    .await;
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &wallet_account_id,
        10,
    )
    .await;
    let app = gateway_router(state);
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/protected-clear-media.mp4");
    write_library_bytes(&app, &token, &uri, b"media").await;

    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &uri,
        crate::protected_content_runtime::RUNTIME_CUSTODY_COMPOSITION_MISSING_MESSAGE,
    )
    .await;
    let mint_root = dir.path().join("protected-content/runtime-mint");
    let journal_entries = std::fs::read_dir(mint_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(journal_entries.len(), 2, "{journal_entries:?}");
    assert!(journal_entries
        .iter()
        .any(|name| name == "runtime-mint-journal.lock"));
    assert!(journal_entries
        .iter()
        .any(|name| name.starts_with("prepare-")));
}

#[tokio::test]
async fn test_runtime_custody_creator_tail_pending_or_failed_never_persists_listing() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", std::sync::Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_content_publish_requests();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();

    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    let uri = format!(
        "{}/Documents/protected-tail-pending",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x81, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &input);
    let mint_id = facts.mint_id;
    let replay_content_cid = facts.content_cid.clone();
    let replay_content_id = facts.content_id.clone();

    let err = runtime_custody_publish_creator_tail_for_test(
        &state,
        &runtime_authority,
        registry.clone(),
        input.clone(),
        facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Runtime custody creator mint is pending exact Wallet or Chain settlement"
    );
    assert_eq!(mock_content_publish_request_count(), 1);
    assert!(
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .is_none()
    );

    let approval_request_id = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let _tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    set_mock_protected_content_chain_receipt_error();
    let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let replay_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &replay_token);
    let replay_facts = crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: replay_content_cid,
        mint_id,
        content_id: replay_content_id,
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({"status": "local_pinned"}),
        receipt: json!({"schema": "elastos.content.availability.receipt/v1"}),
        content_security: json!({"mode": "runtime_custody"}),
    };
    let err = runtime_custody_publish_creator_tail_for_test(
        &state,
        &replay_authority,
        registry,
        input,
        replay_facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Runtime custody creator mint is unavailable"
    );
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await
            .as_deref(),
        Some(approval_request_id.as_str())
    );
    assert_eq!(mock_content_publish_request_count(), 1);
    assert!(
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_runtime_custody_creator_tail_confirmed_replay_is_exact_and_immutable() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", std::sync::Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_content_publish_requests();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();

    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    let uri = format!(
        "{}/Documents/protected-tail-confirmed",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x82, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &input);
    let mint_id = facts.mint_id;
    let replay_content_cid = facts.content_cid.clone();
    let replay_content_id = facts.content_id.clone();

    let pending = runtime_custody_publish_creator_tail_for_test(
        &state,
        &runtime_authority,
        registry.clone(),
        input.clone(),
        facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        pending.to_string(),
        "Runtime custody creator mint is pending exact Wallet or Chain settlement"
    );
    let approval_request_id = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let _tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;

    let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let replay_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &replay_token);
    let replay_facts = crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: replay_content_cid.clone(),
        mint_id,
        content_id: replay_content_id.clone(),
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({"status": "local_pinned"}),
        receipt: json!({"schema": "elastos.content.availability.receipt/v1"}),
        content_security: json!({"mode": "runtime_custody"}),
    };
    let ok = runtime_custody_publish_creator_tail_for_test(
        &state,
        &replay_authority,
        registry.clone(),
        input.clone(),
        replay_facts,
    )
    .await
    .unwrap();
    assert_eq!(ok.mint_id, mint_id);
    let listing =
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .unwrap();
    assert_eq!(listing.cid, replay_content_cid);
    assert_eq!(listing.content_id, replay_content_id);
    assert_eq!(listing.display_name, "protected-tail.mp4");
    assert_eq!(listing.mime_type, input.mime_type);
    assert_eq!(listing.codecs, input.codecs);
    assert_eq!(listing.quantity, "0x2");
    assert_eq!(
        listing.seller_address,
        MOCK_MANAGED_EVM_ADDRESS.to_ascii_lowercase()
    );
    assert_eq!(listing.chain_namespace, "eip155:8453");
    assert_eq!(listing.network, "base-mainnet");
    assert_eq!(
        listing.ledger,
        MOCK_PROTECTED_CONTENT_AUTHORITY_GATEWAY.to_ascii_lowercase()
    );
    assert_eq!(listing.token_id, MOCK_PROTECTED_CONTENT_TOKEN_ID);
    assert_eq!(
        listing.operative,
        MOCK_PROTECTED_CONTENT_OPERATIVE.to_ascii_lowercase()
    );
    assert_eq!(listing.price, MOCK_PROTECTED_CONTENT_LISTING_PRICE);
    assert_eq!(
        listing.pay_token,
        MOCK_PROTECTED_CONTENT_PAY_TOKEN.to_ascii_lowercase()
    );
    assert_eq!(mock_content_publish_request_count(), 1);

    let replay_again_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let replay_again_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &replay_again_token);
    let replay_again_facts = crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: replay_content_cid,
        mint_id,
        content_id: replay_content_id,
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({"status": "local_pinned"}),
        receipt: json!({"schema": "elastos.content.availability.receipt/v1"}),
        content_security: json!({"mode": "runtime_custody"}),
    };
    let replay = runtime_custody_publish_creator_tail_for_test(
        &state,
        &replay_again_authority,
        registry,
        input,
        replay_again_facts,
    )
    .await
    .unwrap();
    assert_eq!(replay.mint_id, mint_id);
    let replayed_listing =
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .unwrap();
    assert_eq!(replayed_listing, listing);
    assert_eq!(mock_content_publish_request_count(), 1);
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await
            .as_deref(),
        Some(approval_request_id.as_str())
    );
}

#[tokio::test]
async fn test_runtime_custody_creator_tail_listing_error_is_unavailable_without_duplicates() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", std::sync::Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_content_publish_requests();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();

    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    let uri = format!(
        "{}/Documents/protected-tail-listing-error",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x83, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &input);
    let mint_id = facts.mint_id;
    let replay_content_cid = facts.content_cid.clone();
    let replay_content_id = facts.content_id.clone();

    let pending = runtime_custody_publish_creator_tail_for_test(
        &state,
        &runtime_authority,
        registry.clone(),
        input.clone(),
        facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        pending.to_string(),
        "Runtime custody creator mint is pending exact Wallet or Chain settlement"
    );
    let approval_request_id = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let initial_mint = crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .load(mint_id)
        .unwrap();
    let initial_effect = initial_mint
        .creator_state()
        .and_then(|state| state.effect())
        .cloned()
        .expect("creator effect bound after pending result");
    let _tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    let signed_transaction = wallet_provider
        .provider
        .latest_transaction_signed_transaction()
        .await
        .expect("completed mock transaction");
    reset_mock_chain_broadcast_count(&signed_transaction);
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 0);
    set_mock_protected_content_chain_listing_error();

    let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let replay_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &replay_token);
    let replay_facts = crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: replay_content_cid.clone(),
        mint_id,
        content_id: replay_content_id.clone(),
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({"status": "local_pinned"}),
        receipt: json!({"schema": "elastos.content.availability.receipt/v1"}),
        content_security: json!({"mode": "runtime_custody"}),
    };
    let err = runtime_custody_publish_creator_tail_for_test(
        &state,
        &replay_authority,
        registry.clone(),
        input.clone(),
        replay_facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Runtime custody creator mint is unavailable"
    );
    assert!(
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(mock_content_publish_request_count(), 1);
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await
            .as_deref(),
        Some(approval_request_id.as_str())
    );
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 1);
    let errored_mint = crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .load(mint_id)
        .unwrap();
    let errored_effect = errored_mint
        .creator_state()
        .and_then(|state| state.effect())
        .cloned()
        .expect("creator effect still bound after listing error");
    assert_eq!(errored_effect.effect_id(), initial_effect.effect_id());
    assert_eq!(
        errored_effect.approval_request_id(),
        initial_effect.approval_request_id()
    );
    assert_eq!(
        errored_effect.request_sha256(),
        initial_effect.request_sha256()
    );

    let replay_again_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let replay_again_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &replay_again_token);
    let replay_again_facts = crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: replay_content_cid,
        mint_id,
        content_id: replay_content_id,
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({"status": "local_pinned"}),
        receipt: json!({"schema": "elastos.content.availability.receipt/v1"}),
        content_security: json!({"mode": "runtime_custody"}),
    };
    let err = runtime_custody_publish_creator_tail_for_test(
        &state,
        &replay_again_authority,
        registry,
        input,
        replay_again_facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Runtime custody creator mint is unavailable"
    );
    assert!(
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(mock_content_publish_request_count(), 1);
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await
            .as_deref(),
        Some(approval_request_id.as_str())
    );
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 1);
    let replayed_mint = crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .load(mint_id)
        .unwrap();
    let replayed_effect = replayed_mint
        .creator_state()
        .and_then(|state| state.effect())
        .cloned()
        .expect("creator effect still bound after listing error replay");
    assert_eq!(replayed_effect.effect_id(), initial_effect.effect_id());
    assert_eq!(
        replayed_effect.approval_request_id(),
        initial_effect.approval_request_id()
    );
    assert_eq!(
        replayed_effect.request_sha256(),
        initial_effect.request_sha256()
    );
}

#[tokio::test]
async fn test_library_provider_runtime_custody_buy_is_denied_before_purchase() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (status, payload) =
        post_library(app, &token, "buy", json!({ "mint_id": "00".repeat(32) })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    assert_eq!(
        payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE
    );
}

#[tokio::test]
async fn test_library_provider_runtime_custody_marketplace_buy_is_denied_before_purchase() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), MARKETPLACE_CAPSULE_ID, &authority);
    let (status, payload) =
        post_library(app, &token, "buy", json!({ "mint_id": "00".repeat(32) })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    assert_eq!(
        payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE
    );
}

#[tokio::test]
async fn test_library_provider_runtime_custody_buy_rejects_caller_supplied_purchase_authority_fields(
) {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), MARKETPLACE_CAPSULE_ID, &authority);
    let (status, payload) = post_library(
        app,
        &token,
        "buy",
        json!({
            "mint_id": "00".repeat(32),
            "account_id": "wallet:eip155:8453:0x19e7e376e7c213b7e7e7e46cc70a5dd086daff2a",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown field"),
        "{payload}"
    );
}

#[tokio::test]
async fn test_runtime_custody_buy_raw_provider_call_requires_gateway_wallet_authority() {
    let dir = tempfile::tempdir().unwrap();
    let response = crate::library::handle_object_provider_runtime_request_with_gateway(
        dir.path(),
        Arc::new(ProviderRegistry::new()),
        &json!({
            "op": "buy",
            "principal_id": "person:local:raw-provider-buy",
            "mint_id": "00".repeat(32),
        }),
        None,
    )
    .await;
    assert_eq!(response["status"], "error");
    assert_eq!(
        response["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE
    );
}

#[tokio::test]
async fn test_runtime_custody_buy_fails_before_effects_when_fresh_availability_is_unavailable() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "buyer");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &wallet_account_id,
        10,
    )
    .await;
    let uri = format!(
        "{}/Documents/protected-buy-no-availability",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let publish_input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x91, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &publish_input);
    seed_runtime_custody_creator_listing_for_buy(
        dir.path(),
        &authority.principal_id,
        &facts,
        MOCK_MANAGED_EVM_ADDRESS,
        false,
    );
    let app = gateway_router(state);
    let (status, payload) = post_library(
        app,
        &token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    assert_eq!(
        payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE
    );
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await,
        None
    );
    assert!(
        crate::protected_content_runtime::load_runtime_custody_purchase(
            dir.path(),
            &authority.principal_id,
            facts.mint_id,
        )
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn test_runtime_custody_buy_native_terminal_replay_is_exact_and_listing_stays_immutable() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    set_mock_protected_content_purchase_native();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "buyer");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &wallet_account_id,
        10,
    )
    .await;
    let uri = format!(
        "{}/Documents/protected-buy-native",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let publish_input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x92, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &publish_input);
    seed_runtime_custody_creator_listing_for_buy(
        dir.path(),
        &authority.principal_id,
        &facts,
        MOCK_MANAGED_EVM_ADDRESS,
        true,
    );
    let listing_path = runtime_custody_listing_path_for_test(dir.path(), facts.mint_id);
    let listing_before = std::fs::read(&listing_path).unwrap();
    let app = gateway_router(state);

    let (pending_status, pending_payload) = post_library(
        app.clone(),
        &token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(pending_status, StatusCode::OK);
    assert_eq!(pending_payload["status"], "error");
    assert_eq!(
        pending_payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
    );
    let approval_request_id = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let _tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    let signed_transaction = wallet_provider
        .provider
        .latest_transaction_signed_transaction()
        .await
        .expect("completed mock transaction");
    reset_mock_chain_broadcast_count(&signed_transaction);
    let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (ok_status, ok_payload) = post_library(
        app.clone(),
        &replay_token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK);
    assert_eq!(ok_payload["status"], "ok");
    assert_eq!(ok_payload["data"]["availability"]["status"], "buyer_owned");
    assert_eq!(
        mock_chain_raw_request_count("resolve_protected_content_purchase"),
        1
    );
    assert_eq!(
        mock_chain_raw_request_count("resolve_protected_content_purchase_access"),
        1
    );
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 1);
    let purchase = crate::protected_content_runtime::load_runtime_custody_purchase(
        dir.path(),
        &authority.principal_id,
        facts.mint_id,
    )
    .unwrap()
    .unwrap();
    let purchase_json = serde_json::to_string(&purchase).unwrap();
    assert!(!purchase_json.contains(&signed_transaction));
    assert!(!purchase_json.contains("\"signed_result\""));
    assert!(!purchase_json.contains("session_id"));
    assert!(!purchase_json.contains("grant_id"));
    assert!(!purchase_json.contains("http://"));

    set_mock_protected_content_listing_quantity("0x0");
    let replacement_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 2)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &replacement_account_id,
        20,
    )
    .await;
    let replay_again_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (replay_status, replay_payload) = post_library(
        app,
        &replay_again_token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(replay_status, StatusCode::OK);
    assert_eq!(replay_payload["status"], "ok");
    assert_eq!(
        replay_payload["data"]["availability"]["status"],
        "buyer_owned"
    );
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await
            .as_deref(),
        Some(approval_request_id.as_str())
    );
    assert_eq!(
        mock_chain_raw_request_count("resolve_protected_content_purchase"),
        1
    );
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 1);
    assert_eq!(std::fs::read(listing_path).unwrap(), listing_before);
    let replayed_purchase = crate::protected_content_runtime::load_runtime_custody_purchase(
        dir.path(),
        &authority.principal_id,
        facts.mint_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(replayed_purchase.account_id, wallet_account_id);
}

#[tokio::test]
async fn test_runtime_custody_buy_erc20_orders_approval_then_buy_without_duplicate_plan() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "buyer");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &wallet_account_id,
        10,
    )
    .await;
    let uri = format!(
        "{}/Documents/protected-buy-erc20",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let publish_input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x93, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &publish_input);
    seed_runtime_custody_creator_listing_for_buy(
        dir.path(),
        &authority.principal_id,
        &facts,
        MOCK_MANAGED_EVM_ADDRESS,
        false,
    );
    let app = gateway_router(state);

    let (pending_one_status, pending_one_payload) = post_library(
        app.clone(),
        &token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(pending_one_status, StatusCode::OK);
    assert_eq!(pending_one_payload["status"], "error");
    assert_eq!(
        pending_one_payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
    );
    let approval_request_one = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let _approval_tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    let signed_approval = wallet_provider
        .provider
        .latest_transaction_signed_transaction()
        .await
        .expect("completed approval transaction");
    reset_mock_chain_broadcast_count(&signed_approval);

    let replay_one_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (pending_two_status, pending_two_payload) = post_library(
        app.clone(),
        &replay_one_token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(pending_two_status, StatusCode::OK);
    assert_eq!(pending_two_payload["status"], "error");
    assert_eq!(
        pending_two_payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
    );
    let approval_request_two = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    assert_ne!(approval_request_one, approval_request_two);
    assert_eq!(mock_chain_broadcast_count(&signed_approval), 1);
    assert_eq!(
        mock_chain_raw_request_count("resolve_protected_content_purchase"),
        1
    );
    let _buy_tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    let signed_buy = wallet_provider
        .provider
        .latest_transaction_signed_transaction()
        .await
        .expect("completed buy transaction");
    reset_mock_chain_broadcast_count(&signed_buy);

    let replay_two_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (ok_status, ok_payload) = post_library(
        app.clone(),
        &replay_two_token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK);
    assert_eq!(ok_payload["status"], "ok");
    assert_eq!(ok_payload["data"]["availability"]["status"], "buyer_owned");
    assert_eq!(mock_chain_broadcast_count(&signed_approval), 1);
    assert_eq!(mock_chain_broadcast_count(&signed_buy), 1);
    assert_eq!(
        mock_chain_raw_request_count("resolve_protected_content_purchase"),
        1
    );

    let replay_again_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (replay_status, replay_payload) = post_library(
        app,
        &replay_again_token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(replay_status, StatusCode::OK);
    assert_eq!(replay_payload["status"], "ok");
    assert_eq!(mock_chain_broadcast_count(&signed_approval), 1);
    assert_eq!(mock_chain_broadcast_count(&signed_buy), 1);
    assert_eq!(
        mock_chain_raw_request_count("resolve_protected_content_purchase"),
        1
    );
}

#[tokio::test]
async fn test_runtime_custody_buy_access_corroboration_stays_nonterminal_until_allow() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    set_mock_protected_content_purchase_native();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "buyer");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &wallet_account_id,
        10,
    )
    .await;
    let uri = format!(
        "{}/Documents/protected-buy-access-pending",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let publish_input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x94, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &publish_input);
    seed_runtime_custody_creator_listing_for_buy(
        dir.path(),
        &authority.principal_id,
        &facts,
        MOCK_MANAGED_EVM_ADDRESS,
        true,
    );
    let app = gateway_router(state);
    let _ = post_library(
        app.clone(),
        &token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    let _tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    let signed_transaction = wallet_provider
        .provider
        .latest_transaction_signed_transaction()
        .await
        .expect("completed mock transaction");
    reset_mock_chain_broadcast_count(&signed_transaction);

    for set_mode in [
        set_mock_protected_content_purchase_access_denied as fn(),
        set_mock_protected_content_purchase_access_error as fn(),
    ] {
        reset_mock_protected_content_purchase_fixture();
        set_mock_protected_content_purchase_native();
        set_mode();
        let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
        let (status, payload) = post_library(
            app.clone(),
            &replay_token,
            "buy",
            json!({
                "mint_id": hex::encode(facts.mint_id.as_bytes()),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["status"], "error");
        assert_eq!(
            payload["message"],
            crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
        );
        let purchase = crate::protected_content_runtime::load_runtime_custody_purchase(
            dir.path(),
            &authority.principal_id,
            facts.mint_id,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            purchase.progress,
            crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
                confirmed_buy: Some(_)
            }
        ));
    }

    reset_mock_protected_content_purchase_fixture();
    set_mock_protected_content_purchase_native();
    let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let (ok_status, ok_payload) = post_library(
        app,
        &replay_token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(ok_status, StatusCode::OK);
    assert_eq!(ok_payload["status"], "ok");
    assert_eq!(ok_payload["data"]["availability"]["status"], "buyer_owned");
}

#[tokio::test]
async fn test_runtime_custody_buy_selects_latest_chain_transaction_default() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    set_mock_protected_content_purchase_native();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "buyer");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let older_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 1)
        .await;
    let latest_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 2)
        .await;
    let other_chain_address = mock_managed_evm_address(3).unwrap();
    let other_chain_account_id = format!("wallet:eip155:20:{other_chain_address}");
    wallet_provider.provider.accounts.lock().await.push(json!({
        "account_id": other_chain_account_id,
        "principal_id": authority.principal_id,
        "proof_binding_id": format!("proof:wallet:managed:eip155:20:{other_chain_address}"),
        "chain_namespace": "eip155:20",
        "address": other_chain_address,
        "proof_type": "managed_evm",
        "signing_available": true,
        "signing_status": "managed_key_available",
        "label": "Managed",
        "linked_at": crate::auth::now_ts()
    }));
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:20",
        &other_chain_account_id,
        30,
    )
    .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &older_account_id,
        10,
    )
    .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &latest_account_id,
        20,
    )
    .await;
    let uri = format!(
        "{}/Documents/protected-buy-select-default",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let publish_input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x95, &older_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &publish_input);
    seed_runtime_custody_creator_listing_for_buy(
        dir.path(),
        &authority.principal_id,
        &facts,
        MOCK_MANAGED_EVM_ADDRESS,
        true,
    );
    let app = gateway_router(state);

    let (pending_status, pending_payload) = post_library(
        app,
        &token,
        "buy",
        json!({
            "mint_id": hex::encode(facts.mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(pending_status, StatusCode::OK);
    assert_eq!(pending_payload["status"], "error");
    assert_eq!(
        pending_payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
    );
    let purchase = crate::protected_content_runtime::load_runtime_custody_purchase(
        dir.path(),
        &authority.principal_id,
        facts.mint_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(purchase.account_id, latest_account_id);
    assert_eq!(purchase.chain_namespace, "eip155:8453");
}

#[tokio::test]
async fn test_runtime_custody_creator_publish_binding_selects_latest_chain_transaction_default() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "creator");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let older_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 1)
        .await;
    let latest_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 2)
        .await;
    let latest_address = mock_managed_evm_address(2).unwrap();
    let other_chain_address = mock_managed_evm_address(3).unwrap();
    let other_chain_account_id = format!("wallet:eip155:20:{other_chain_address}");
    wallet_provider.provider.accounts.lock().await.push(json!({
        "account_id": other_chain_account_id,
        "principal_id": authority.principal_id,
        "proof_binding_id": format!("proof:wallet:managed:eip155:20:{other_chain_address}"),
        "chain_namespace": "eip155:20",
        "address": other_chain_address,
        "proof_type": "managed_evm",
        "signing_available": true,
        "signing_status": "managed_key_available",
        "label": "Managed",
        "linked_at": crate::auth::now_ts()
    }));
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:20",
        &other_chain_account_id,
        30,
    )
    .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &older_account_id,
        10,
    )
    .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &latest_account_id,
        20,
    )
    .await;

    let binding = crate::api::gateway::resolve_runtime_custody_creator_publish_binding(
        &state,
        &runtime_authority,
        &authority.principal_id,
        &format!(
            "{}/Documents/protected-select-default",
            crate::auth::principal_localhost_root(&authority.principal_id)
        ),
        "plain_localhost_root",
    )
    .await
    .unwrap();
    assert_eq!(binding.account_id, latest_account_id);
    assert_eq!(binding.address, latest_address.to_ascii_lowercase());
}

#[tokio::test]
async fn test_runtime_custody_creator_publish_binding_reuses_media_preparation_authority() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "creator");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let bound_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 1)
        .await;
    let newer_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 2)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &authority.principal_id,
        "eip155:8453",
        &newer_account_id,
        20,
    )
    .await;
    let object_uri = format!(
        "{}/Documents/protected-media-bound-default",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let protected_content_root = dir.path().join("protected-content");
    std::fs::create_dir_all(&protected_content_root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            &protected_content_root,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let preparation = elastos_protected_content_runtime::RuntimeMediaPreparationRecord::new(
        &authority.principal_id,
        &object_uri,
        "plain_localhost_root",
        elastos_protected_content_contracts::Digest32::new([0x51; 32]),
        "media",
        &bound_account_id,
        mock_managed_evm_address(1).unwrap(),
        runtime_custody_creator_source_digest(),
    )
    .unwrap();
    crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .persist_media_preparation(&preparation)
        .unwrap();

    let binding = crate::api::gateway::resolve_runtime_custody_creator_publish_binding(
        &state,
        &runtime_authority,
        &authority.principal_id,
        &object_uri,
        "plain_localhost_root",
    )
    .await
    .unwrap();
    assert_eq!(binding.account_id, bound_account_id);
    assert_eq!(binding.address, mock_managed_evm_address(1).unwrap());
}

#[tokio::test]
async fn test_runtime_custody_creator_publish_binding_requires_a_valid_chain_transaction_default() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "creator");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let valid_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 1)
        .await;
    let other_valid_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 2)
        .await;
    let object_uri = format!(
        "{}/Documents/protected-invalid-default",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );

    let cases = [
        ("missing", vec![], None),
        (
            "wrong_chain",
            vec![json!({
                "schema": "elastos.wallet.default_account/v1",
                "principal_id": authority.principal_id,
                "chain_namespace": "eip155:20",
                "intent": "transaction_intent",
                "account_id": valid_account_id,
                "set_at": 10,
            })],
            None,
        ),
        (
            "stale",
            vec![json!({
                "schema": "elastos.wallet.default_account/v1",
                "principal_id": authority.principal_id,
                "chain_namespace": "eip155:8453",
                "intent": "transaction_intent",
                "account_id": "wallet:eip155:8453:0x0000000000000000000000000000000000000000",
                "set_at": 10,
            })],
            None,
        ),
        (
            "ambiguous",
            vec![
                json!({
                    "schema": "elastos.wallet.default_account/v1",
                    "principal_id": authority.principal_id,
                    "chain_namespace": "eip155:8453",
                    "intent": "transaction_intent",
                    "account_id": valid_account_id,
                    "set_at": 10,
                }),
                json!({
                    "schema": "elastos.wallet.default_account/v1",
                    "principal_id": authority.principal_id,
                    "chain_namespace": "eip155:8453",
                    "intent": "transaction_intent",
                    "account_id": other_valid_account_id,
                    "set_at": 10,
                }),
            ],
            None,
        ),
        (
            "external_only",
            vec![json!({
                "schema": "elastos.wallet.default_account/v1",
                "principal_id": authority.principal_id,
                "chain_namespace": "eip155:8453",
                "intent": "transaction_intent",
                "account_id": valid_account_id,
                "set_at": 10,
            })],
            Some(json!({
                "account_id": valid_account_id,
                "principal_id": authority.principal_id,
                "proof_binding_id": format!("proof:wallet:connector:eip155:8453:{}", mock_managed_evm_address(1).unwrap()),
                "chain_namespace": "eip155:8453",
                "address": mock_managed_evm_address(1).unwrap(),
                "proof_type": "connector_evm",
                "signing_available": false,
                "signing_status": "connector_only",
                "label": "External",
                "linked_at": crate::auth::now_ts(),
            })),
        ),
    ];

    for (label, defaults, replacement_account) in cases {
        wallet_provider.provider.defaults.lock().await.clear();
        wallet_provider
            .provider
            .accounts
            .lock()
            .await
            .retain(|account| account["proof_type"].as_str() != Some("connector_evm"));
        wallet_provider
            .provider
            .defaults
            .lock()
            .await
            .extend(defaults);
        if let Some(account) = replacement_account {
            let mut accounts = wallet_provider.provider.accounts.lock().await;
            if let Some(existing) = accounts.iter_mut().find(|existing| {
                existing.get("account_id").and_then(Value::as_str)
                    == account.get("account_id").and_then(Value::as_str)
            }) {
                *existing = account;
            }
        }
        let error = crate::api::gateway::resolve_runtime_custody_creator_publish_binding(
            &state,
            &runtime_authority,
            &authority.principal_id,
            &object_uri,
            "plain_localhost_root",
        )
        .await
        .expect_err("invalid creator default must fail closed");
        assert_eq!(
            error.to_string(),
            "Runtime custody creator mint is unavailable",
            "{label}"
        );
    }
}

#[tokio::test]
async fn test_runtime_custody_buy_requires_a_valid_chain_transaction_default() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_profile(dir.path(), "buyer");
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let valid_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 1)
        .await;
    let other_valid_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&authority.principal_id, 2)
        .await;
    let uri = format!(
        "{}/Documents/protected-buy-invalid-default",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let publish_input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x96, &valid_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &publish_input);
    seed_runtime_custody_creator_listing_for_buy(
        dir.path(),
        &authority.principal_id,
        &facts,
        MOCK_MANAGED_EVM_ADDRESS,
        true,
    );
    let app = gateway_router(state);

    let cases = [
        ("missing", vec![], None),
        (
            "wrong_chain",
            vec![json!({
                "schema": "elastos.wallet.default_account/v1",
                "principal_id": authority.principal_id,
                "chain_namespace": "eip155:20",
                "intent": "transaction_intent",
                "account_id": valid_account_id,
                "set_at": 10,
            })],
            None,
        ),
        (
            "stale",
            vec![json!({
                "schema": "elastos.wallet.default_account/v1",
                "principal_id": authority.principal_id,
                "chain_namespace": "eip155:8453",
                "intent": "transaction_intent",
                "account_id": "wallet:eip155:8453:0x0000000000000000000000000000000000000000",
                "set_at": 10,
            })],
            None,
        ),
        (
            "ambiguous",
            vec![
                json!({
                    "schema": "elastos.wallet.default_account/v1",
                    "principal_id": authority.principal_id,
                    "chain_namespace": "eip155:8453",
                    "intent": "transaction_intent",
                    "account_id": valid_account_id,
                    "set_at": 10,
                }),
                json!({
                    "schema": "elastos.wallet.default_account/v1",
                    "principal_id": authority.principal_id,
                    "chain_namespace": "eip155:8453",
                    "intent": "transaction_intent",
                    "account_id": other_valid_account_id,
                    "set_at": 10,
                }),
            ],
            None,
        ),
        (
            "external_only",
            vec![json!({
                "schema": "elastos.wallet.default_account/v1",
                "principal_id": authority.principal_id,
                "chain_namespace": "eip155:8453",
                "intent": "transaction_intent",
                "account_id": valid_account_id,
                "set_at": 10,
            })],
            Some(json!({
                "account_id": valid_account_id,
                "principal_id": authority.principal_id,
                "proof_binding_id": format!("proof:wallet:connector:eip155:8453:{}", mock_managed_evm_address(1).unwrap()),
                "chain_namespace": "eip155:8453",
                "address": mock_managed_evm_address(1).unwrap(),
                "proof_type": "connector_evm",
                "signing_available": false,
                "signing_status": "connector_only",
                "label": "External",
                "linked_at": crate::auth::now_ts(),
            })),
        ),
    ];

    for (_name, defaults, replacement_account) in cases {
        {
            let mut stored_defaults = wallet_provider.provider.defaults.lock().await;
            stored_defaults.clear();
            stored_defaults.extend(defaults);
        }
        if let Some(replacement_account) = replacement_account.clone() {
            let mut accounts = wallet_provider.provider.accounts.lock().await;
            if let Some(existing) = accounts.iter_mut().find(|account| {
                account.get("account_id").and_then(Value::as_str)
                    == replacement_account
                        .get("account_id")
                        .and_then(Value::as_str)
            }) {
                *existing = replacement_account;
            }
        }
        wallet_provider.clear_requests().await;
        let (status, payload) = post_library(
            app.clone(),
            &token,
            "buy",
            json!({
                "mint_id": hex::encode(facts.mint_id.as_bytes()),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payload["status"], "error");
        assert_eq!(
            payload["message"],
            crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE
        );
        assert_eq!(
            wallet_provider
                .provider
                .latest_transaction_approval_request_id()
                .await,
            None
        );
        assert!(
            crate::protected_content_runtime::load_runtime_custody_purchase(
                dir.path(),
                &authority.principal_id,
                facts.mint_id,
            )
            .unwrap()
            .is_none()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_runtime_custody_typed_publish_buy_open_read_segment_and_close() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    crate::protected_content_runtime::tests::write_device_key(dir.path(), 0x5a);
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    reset_mock_content_publish_requests();
    reset_mock_chain_raw_requests();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    registry
        .register_sub_provider("content", std::sync::Arc::new(MockContentProvider))
        .await
        .unwrap();
    registry
        .register_sub_provider(
            "object",
            std::sync::Arc::new(crate::library::ObjectProvider::new(
                dir.path().to_path_buf(),
                std::sync::Arc::downgrade(&registry),
            )),
        )
        .await
        .unwrap();

    let _process_fixture = crate::protected_content_runtime::tests::register_runtime_custody_process_providers_for_test_registry(
        dir.path(),
        &registry,
    )
    .await;
    crate::protected_content_runtime::tests::register_runtime_custody_mock_media_provider_for_test_registry(
        dir.path(),
        &registry,
    )
    .await;
    let creator = passkey_authority_with_profile_role_credential(
        dir.path(),
        "creator",
        crate::auth::RuntimePrincipalRole::Admin,
        "gateway-test-passkey-creator",
    );
    let buyer = passkey_authority_with_profile_role_credential(
        dir.path(),
        "buyer",
        crate::auth::RuntimePrincipalRole::Admin,
        "gateway-test-passkey-buyer",
    );
    let creator_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &creator);
    let buyer_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &buyer);
    let player_token = projection_launch_token_for_authority_context(
        dir.path(),
        ELACITY_PLAYER_CAPSULE_ID_FOR_TEST,
        &buyer,
    );
    let creator_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&creator.principal_id, 1)
        .await;
    let replacement_creator_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&creator.principal_id, 3)
        .await;
    let buyer_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal_with_index(&buyer.principal_id, 2)
        .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &creator.principal_id,
        "eip155:8453",
        &creator_account_id,
        10,
    )
    .await;
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &buyer.principal_id,
        "eip155:8453",
        &buyer_account_id,
        10,
    )
    .await;
    let app = gateway_router(state.clone());

    let creator_root = crate::auth::principal_localhost_root(&creator.principal_id);
    let uri = format!("{creator_root}/Documents/protected-runtime-proof.mp4");
    let (clear_init, clear_segments) =
        crate::protected_content_runtime::tests::runtime_custody_gateway_media_output_for_test();
    write_library_bytes(&app, &creator_token, &uri, b"media").await;
    let publish_body = json!({
        "uri": uri,
        "protection": {
            "mode": "runtime_custody",
            "copies": "0x2",
            "price": MOCK_PROTECTED_CONTENT_LISTING_PRICE,
        },
    });
    let (publish_pending_status, publish_pending) =
        post_library(app.clone(), &creator_token, "publish", publish_body.clone()).await;
    assert_eq!(publish_pending_status, StatusCode::OK);
    assert_eq!(publish_pending["status"], "error");
    assert_eq!(
        publish_pending["message"],
        "Runtime custody creator mint is pending exact Wallet or Chain settlement"
    );
    set_mock_wallet_transaction_default(
        &wallet_provider.provider,
        &creator.principal_id,
        "eip155:8453",
        &replacement_creator_account_id,
        20,
    )
    .await;
    let creator_signed_transaction = {
        let _ = wallet_provider
            .provider
            .complete_latest_transaction_approval()
            .await;
        wallet_provider
            .provider
            .latest_transaction_signed_transaction()
            .await
            .expect("completed creator transaction")
    };
    reset_mock_chain_broadcast_count(&creator_signed_transaction);

    let (publish_ok_status, publish_ok) =
        post_library(app.clone(), &creator_token, "publish", publish_body).await;
    assert_eq!(publish_ok_status, StatusCode::OK);
    assert_eq!(publish_ok["status"], "ok", "{publish_ok}");
    let mint_id_hex = publish_ok["data"]["content_security"]["mint_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mint_id = elastos_protected_content_contracts::Digest32::new(
        hex::decode(&mint_id_hex).unwrap().try_into().unwrap(),
    );
    let persisted_mint = crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .load(mint_id)
        .unwrap();
    assert_eq!(
        persisted_mint
            .creator_state()
            .unwrap()
            .desired_terms()
            .wallet_account_id(),
        creator_account_id
    );
    let listing_path = runtime_custody_listing_path_for_test(dir.path(), mint_id);
    let listing_before_buy = std::fs::read(&listing_path).unwrap();
    assert_eq!(mock_content_publish_request_count(), 2);
    assert_eq!(mock_chain_broadcast_count(&creator_signed_transaction), 1);

    let (buy_pending_status, buy_pending) = post_library(
        app.clone(),
        &buyer_token,
        "buy",
        json!({
            "mint_id": mint_id_hex,
        }),
    )
    .await;
    assert_eq!(buy_pending_status, StatusCode::OK);
    assert_eq!(buy_pending["status"], "error");
    assert_eq!(
        buy_pending["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
    );
    let buyer_approval_request_one = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let buyer_signed_approval = {
        let _ = wallet_provider
            .provider
            .complete_latest_transaction_approval()
            .await;
        wallet_provider
            .provider
            .latest_transaction_signed_transaction()
            .await
            .expect("completed buyer approval transaction")
    };
    reset_mock_chain_broadcast_count(&buyer_signed_approval);

    let (buy_pending_two_status, buy_pending_two) = post_library(
        app.clone(),
        &buyer_token,
        "buy",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(buy_pending_two_status, StatusCode::OK);
    assert_eq!(buy_pending_two["status"], "error");
    assert_eq!(
        buy_pending_two["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_PENDING_MESSAGE
    );
    let buyer_approval_request_two = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    assert_ne!(buyer_approval_request_one, buyer_approval_request_two);
    assert_eq!(mock_chain_broadcast_count(&buyer_signed_approval), 1);

    let buyer_signed_transaction = {
        let _ = wallet_provider
            .provider
            .complete_latest_transaction_approval()
            .await;
        wallet_provider
            .provider
            .latest_transaction_signed_transaction()
            .await
            .expect("completed buyer transaction")
    };
    reset_mock_chain_broadcast_count(&buyer_signed_transaction);

    let (buy_ok_status, buy_ok) = post_library(
        app.clone(),
        &buyer_token,
        "buy",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
        }),
    )
    .await;
    assert_eq!(buy_ok_status, StatusCode::OK);
    assert_eq!(buy_ok["status"], "ok", "{buy_ok}");
    assert_eq!(buy_ok["data"]["availability"]["status"], "buyer_owned");
    assert_eq!(mock_chain_broadcast_count(&buyer_signed_approval), 1);
    assert_eq!(mock_chain_broadcast_count(&buyer_signed_transaction), 1);
    assert_eq!(std::fs::read(&listing_path).unwrap(), listing_before_buy);

    let purchase = crate::protected_content_runtime::load_runtime_custody_purchase(
        dir.path(),
        &buyer.principal_id,
        mint_id,
    )
    .unwrap()
    .expect("terminal purchase record");
    assert_eq!(purchase.principal_id, buyer.principal_id);
    assert_eq!(
        purchase.account_id,
        format!(
            "wallet:eip155:8453:{}",
            mock_managed_evm_address(2).unwrap()
        )
    );
    assert!(matches!(
        purchase.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ));

    wallet_provider.clear_requests().await;
    let (open_status, open_payload) = post_library(
        app.clone(),
        &player_token,
        "open_viewer",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
            "principal_id": "person:substituted",
            "launch_id": "launch:ffffffffffffffffffffffffffffffff",
            "proof_binding_id": "proof:substituted",
            "session_id": "runtime-session:substituted",
            "grant_id": "grant:substituted",
        }),
    )
    .await;
    let open_wallet_ops = wallet_provider.recorded_v2_operation_kinds().await;
    assert_eq!(open_status, StatusCode::OK);
    assert_eq!(
        open_payload["status"], "ok",
        "wallet_ops={open_wallet_ops:?} payload={open_payload}"
    );
    assert_eq!(
        open_wallet_ops,
        vec![WalletOperationKind::RequestProtectedContentRightsSignature]
    );
    let viewer_handle = open_payload["data"]["viewer_session_handle"]
        .as_str()
        .unwrap()
        .to_string();
    let open_keys = open_payload["data"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        open_keys,
        std::collections::BTreeSet::from([
            "codecs",
            "expires_at",
            "has_init_segment",
            "mime_type",
            "mint_id",
            "schema",
            "segment_count",
            "viewer_session_handle",
        ])
    );
    assert_eq!(
        open_payload["data"]["segment_count"].as_u64(),
        Some(clear_segments.len() as u64)
    );

    let substituted_player_token = projection_launch_token_for_authority_context(
        dir.path(),
        ELACITY_PLAYER_CAPSULE_ID_FOR_TEST,
        &buyer,
    );
    let (substituted_launch_status, substituted_launch_payload) = post_library(
        app.clone(),
        &substituted_player_token,
        "read_viewer",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
            "viewer_session_handle": viewer_handle.clone(),
        }),
    )
    .await;
    assert_eq!(substituted_launch_status, StatusCode::OK);
    assert_eq!(substituted_launch_payload["status"], "error");
    assert_eq!(
        substituted_launch_payload["message"],
        "Runtime custody viewer session is unavailable"
    );

    let (init_status, init_payload) = post_library(
        app.clone(),
        &player_token,
        "read_viewer",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
            "viewer_session_handle": viewer_handle,
            "principal_id": "person:substituted",
            "proof_binding_id": "proof:substituted",
            "session_id": "runtime-session:substituted",
            "grant_id": "grant:substituted",
        }),
    )
    .await;
    assert_eq!(init_status, StatusCode::OK);
    assert_eq!(init_payload["status"], "ok", "{init_payload}");
    let init_bytes = base64::engine::general_purpose::STANDARD
        .decode(init_payload["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(init_bytes, clear_init);

    let (segment_status, segment_payload) = post_library(
        app.clone(),
        &player_token,
        "read_viewer",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
            "viewer_session_handle": open_payload["data"]["viewer_session_handle"],
            "principal_id": "person:substituted",
            "proof_binding_id": "proof:substituted",
            "session_id": "runtime-session:substituted",
            "grant_id": "grant:substituted",
            "segment_index": 0,
        }),
    )
    .await;
    assert_eq!(segment_status, StatusCode::OK);
    assert_eq!(segment_payload["status"], "ok", "{segment_payload}");
    let segment_bytes = base64::engine::general_purpose::STANDARD
        .decode(segment_payload["data"]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(segment_bytes, clear_segments[0]);

    let (close_status, close_payload) = post_library(
        app,
        &player_token,
        "close_viewer",
        json!({
            "mint_id": hex::encode(mint_id.as_bytes()),
            "viewer_session_handle": open_payload["data"]["viewer_session_handle"],
            "principal_id": "person:substituted",
            "proof_binding_id": "proof:substituted",
            "session_id": "runtime-session:substituted",
            "grant_id": "grant:substituted",
        }),
    )
    .await;
    assert_eq!(close_status, StatusCode::OK);
    assert_eq!(close_payload["status"], "ok", "{close_payload}");

    let viewer_record_path =
        runtime_custody_viewer_record_path_for_test(dir.path(), &buyer.principal_id, mint_id);
    let viewer_record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&viewer_record_path).unwrap()).unwrap();
    assert_eq!(viewer_record["lifecycle_status"], "closed");
    assert!(viewer_record["pending_close_result"].is_null());
    assert!(viewer_record["pending_cancel_result"].is_null());
    assert_eq!(std::fs::read(&listing_path).unwrap(), listing_before_buy);
}

#[tokio::test]
async fn test_runtime_custody_creator_tail_rejects_resolved_source_drift_before_chain_effect() {
    let _guard = protected_content_gateway_mock_test_guard().lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let registry = state.provider_registry.as_ref().unwrap().clone();
    registry
        .register_sub_provider("content", std::sync::Arc::new(MockContentProvider))
        .await
        .unwrap();
    reset_mock_content_publish_requests();
    reset_mock_protected_content_chain_mode();
    reset_mock_protected_content_purchase_fixture();
    reset_mock_chain_raw_requests();

    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let runtime_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &token);
    let wallet_account_id = wallet_provider
        .provider
        .seed_managed_evm_account_for_principal(&authority.principal_id)
        .await;
    let uri = format!(
        "{}/Documents/protected-tail-source-drift",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let input =
        runtime_custody_creator_test_input(&authority.principal_id, &uri, 0x84, &wallet_account_id);
    let facts = seed_completed_runtime_custody_mint(dir.path(), &input);
    let mint_id = facts.mint_id;
    let replay_content_cid = facts.content_cid.clone();
    let replay_content_id = facts.content_id.clone();

    let pending = runtime_custody_publish_creator_tail_for_test(
        &state,
        &runtime_authority,
        registry.clone(),
        input.clone(),
        facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        pending.to_string(),
        "Runtime custody creator mint is pending exact Wallet or Chain settlement"
    );
    let approval_request_id = wallet_provider
        .provider
        .latest_transaction_approval_request_id()
        .await
        .unwrap();
    let initial_mint = crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .load(mint_id)
        .unwrap();
    let initial_effect = initial_mint
        .creator_state()
        .and_then(|state| state.effect())
        .cloned()
        .expect("creator effect bound after pending result");
    let _tx_hash = wallet_provider
        .provider
        .complete_latest_transaction_approval()
        .await;
    let signed_transaction = wallet_provider
        .provider
        .latest_transaction_signed_transaction()
        .await
        .expect("completed mock transaction");
    reset_mock_chain_broadcast_count(&signed_transaction);
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 0);
    set_mock_protected_content_chain_creator_mint_resolve_drift();

    let replay_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let replay_authority =
        runtime_wallet_authority_for_app_token(dir.path(), LIBRARY_CAPSULE_ID, &replay_token);
    let replay_facts = crate::protected_content_runtime::RuntimeCustodyLibraryPublishFacts {
        content_cid: replay_content_cid,
        mint_id,
        content_id: replay_content_id,
        display_name: "protected-tail.mp4".to_string(),
        mime_type: input.mime_type.clone(),
        codecs: input.codecs.clone(),
        availability: json!({"status": "local_pinned"}),
        receipt: json!({"schema": "elastos.content.availability.receipt/v1"}),
        content_security: json!({"mode": "runtime_custody"}),
    };
    let err = runtime_custody_publish_creator_tail_for_test(
        &state,
        &replay_authority,
        registry.clone(),
        input,
        replay_facts,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Runtime custody creator mint is unavailable"
    );
    assert_eq!(mock_chain_broadcast_count(&signed_transaction), 0);
    assert_eq!(mock_content_publish_request_count(), 1);
    assert!(
        crate::protected_content_runtime::load_runtime_custody_listing(dir.path(), mint_id)
            .unwrap()
            .is_none()
    );
    let reloaded = crate::protected_content_runtime::runtime_mint_journal(dir.path())
        .load(mint_id)
        .unwrap();
    assert!(
        reloaded
            .creator_state()
            .and_then(|state| state.effect())
            .cloned()
            == Some(initial_effect)
    );
    assert_eq!(
        wallet_provider
            .provider
            .latest_transaction_approval_request_id()
            .await
            .as_deref(),
        Some(approval_request_id.as_str())
    );
}

#[tokio::test]
async fn test_library_provider_runtime_custody_open_is_denied_before_purchase() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), ELACITY_PLAYER_CAPSULE_ID_FOR_TEST, &authority);
    let (status, payload) = post_library(
        app,
        &token,
        "open_viewer",
        json!({
            "mint_id": "00".repeat(32),
            "proof_binding_id": "caller-selected-proof",
            "session_id": "caller-selected-session",
            "grant_id": "caller-selected-grant",
            "wallet_request_hex": "00",
            "wallet_response_hex": "00",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["status"], "error");
    assert_eq!(
        payload["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE
    );
}

#[tokio::test]
async fn test_library_provider_runtime_custody_viewer_ops_require_player_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let library_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let denied_message = "home launch token is not authorized for this provider";

    for (op, request) in [
        (
            "open_viewer",
            json!({
                "mint_id": "00".repeat(32),
            }),
        ),
        (
            "read_viewer",
            json!({
                "mint_id": "00".repeat(32),
                "viewer_session_handle": "00".repeat(32),
            }),
        ),
        (
            "close_viewer",
            json!({
                "mint_id": "00".repeat(32),
                "viewer_session_handle": "00".repeat(32),
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri(format!("/api/provider/object/{op}"))
                    .header("x-elastos-home-token", library_token.clone())
                    .header(CONTENT_TYPE, "application/json")
                    .body(provider_body(request))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(status, StatusCode::FORBIDDEN, "{op} body={body}");
        assert_eq!(body, denied_message, "{op} body={body}");
    }

    let wrong_resource_token = issue_home_projection_launch_token_with_context(
        dir.path(),
        LIBRARY_CAPSULE_ID,
        ELACITY_PLAYER_CAPSULE_ID_FOR_TEST,
        &HomeLaunchTokenContext {
            principal_id: authority.principal_id.clone(),
            session_id: authority.session_id.clone(),
            proof_binding_id: Some(authority.proof_binding_id.clone()),
            grant_id: authority.grant_id.clone(),
        },
    )
    .unwrap();
    let wrong_resource = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/open_viewer")
                .header("x-elastos-home-token", wrong_resource_token)
                .header(CONTENT_TYPE, "application/json")
                .body(provider_body(json!({ "mint_id": "00".repeat(32) })))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_resource.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        String::from_utf8(
            axum::body::to_bytes(wrong_resource.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec()
        )
        .unwrap(),
        "home launch token is not authorized for this viewer"
    );

    let player_token = projection_launch_token_for_authority_context(
        dir.path(),
        ELACITY_PLAYER_CAPSULE_ID_FOR_TEST,
        &authority,
    );
    let malformed_token = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/open_viewer")
                .header("x-elastos-home-token", "not-an-opaque-launch-token")
                .header(CONTENT_TYPE, "application/json")
                .body(provider_body(json!({ "mint_id": "00".repeat(32) })))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed_token.status(), StatusCode::FORBIDDEN);

    let wrong_origin = app
        .oneshot(
            test_browser_request("localhost:61180", "https://example.invalid")
                .method("POST")
                .uri("/api/provider/object/open_viewer")
                .header("x-elastos-home-token", player_token)
                .header(CONTENT_TYPE, "application/json")
                .body(provider_body(json!({ "mint_id": "00".repeat(32) })))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_library_provider_runtime_custody_publish_rejects_invalid_input_layouts() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let invalid_message = "Runtime custody publish input invalid";

    let file_uri = format!("{root}/Documents/protected-directory-target");
    std::fs::create_dir_all(library_object_path(dir.path(), &file_uri)).unwrap();
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &file_uri,
        invalid_message,
    )
    .await;

    let missing_init_uri = format!("{root}/Documents/protected-missing-init");
    let missing_init_path = library_object_path(dir.path(), &missing_init_uri);
    std::fs::create_dir_all(missing_init_path.join("segments")).unwrap();
    let (_, missing_init_segments) = clear_runtime_custody_media(0x52);
    write_library_bytes(
        &app,
        &token,
        &format!("{missing_init_uri}/segments/00000000.m4s"),
        &missing_init_segments[0],
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &missing_init_uri,
        invalid_message,
    )
    .await;

    let missing_segments_uri = format!("{root}/Documents/protected-missing-segments");
    let (missing_segments_init, _) = clear_runtime_custody_media(0x52);
    write_library_bytes(
        &app,
        &token,
        &format!("{missing_segments_uri}/init.mp4"),
        &missing_segments_init,
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &missing_segments_uri,
        invalid_message,
    )
    .await;

    let empty_segments_uri = format!("{root}/Documents/protected-empty-segments");
    let empty_segments_path = library_object_path(dir.path(), &empty_segments_uri);
    std::fs::create_dir_all(empty_segments_path.join("segments")).unwrap();
    let (empty_segments_init, _) = clear_runtime_custody_media(0x53);
    write_library_bytes(
        &app,
        &token,
        &format!("{empty_segments_uri}/init.mp4"),
        &empty_segments_init,
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &empty_segments_uri,
        invalid_message,
    )
    .await;

    let empty_segment_uri = format!("{root}/Documents/protected-empty-segment");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &empty_segment_uri, 0x54)
        .await;
    write_library_bytes(
        &app,
        &token,
        &format!("{empty_segment_uri}/segments/00000000.m4s"),
        &[],
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &empty_segment_uri,
        invalid_message,
    )
    .await;

    let gap_uri = format!("{root}/Documents/protected-gap");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &gap_uri, 0x55).await;
    std::fs::remove_file(library_object_path(
        dir.path(),
        &format!("{gap_uri}/segments/00000000.m4s"),
    ))
    .unwrap();
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &gap_uri,
        invalid_message,
    )
    .await;

    let wrong_name_uri = format!("{root}/Documents/protected-wrong-name");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &wrong_name_uri, 0x56).await;
    std::fs::rename(
        library_object_path(
            dir.path(),
            &format!("{wrong_name_uri}/segments/00000000.m4s"),
        ),
        library_object_path(
            dir.path(),
            &format!("{wrong_name_uri}/segments/segment0.m4s"),
        ),
    )
    .unwrap();
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &wrong_name_uri,
        invalid_message,
    )
    .await;

    let extra_root_uri = format!("{root}/Documents/protected-extra-root-entry");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &extra_root_uri, 0x57).await;
    write_library_bytes(
        &app,
        &token,
        &format!("{extra_root_uri}/notes.txt"),
        b"extra root entry",
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &extra_root_uri,
        invalid_message,
    )
    .await;

    let symlink_uri = format!("{root}/Documents/protected-symlink");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &symlink_uri, 0x58).await;
    let symlink_path =
        library_object_path(dir.path(), &format!("{symlink_uri}/segments/00000000.m4s"));
    std::fs::remove_file(&symlink_path).unwrap();
    let symlink_target = dir.path().join("symlink-target.bin");
    std::fs::write(&symlink_target, b"symlink-target").unwrap();
    std::os::unix::fs::symlink(&symlink_target, &symlink_path).unwrap();
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &symlink_uri,
        invalid_message,
    )
    .await;

    let non_file_uri = format!("{root}/Documents/protected-non-file");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &non_file_uri, 0x59).await;
    let non_file_path =
        library_object_path(dir.path(), &format!("{non_file_uri}/segments/00000000.m4s"));
    std::fs::remove_file(&non_file_path).unwrap();
    std::fs::create_dir_all(&non_file_path).unwrap();
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &non_file_uri,
        invalid_message,
    )
    .await;

    let oversize_uri = format!("{root}/Documents/protected-oversize");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &oversize_uri, 0x5a).await;
    let oversize_segment_path =
        library_object_path(dir.path(), &format!("{oversize_uri}/segments/00000000.m4s"));
    let oversize_file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&oversize_segment_path)
        .unwrap();
    oversize_file
        .set_len(
            elastos_protected_content_provider_contracts::MAX_PROTECT_MEDIA_PART_BYTES_V1 as u64
                + 1,
        )
        .unwrap();
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &oversize_uri,
        invalid_message,
    )
    .await;

    let malformed_init_uri = format!("{root}/Documents/protected-malformed-init");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &malformed_init_uri, 0x5b)
        .await;
    write_library_bytes(
        &app,
        &token,
        &format!("{malformed_init_uri}/init.mp4"),
        b"not an mp4 init",
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &malformed_init_uri,
        invalid_message,
    )
    .await;

    let malformed_segment_uri = format!("{root}/Documents/protected-malformed-segment");
    create_runtime_custody_publish_directory(
        dir.path(),
        &app,
        &token,
        &malformed_segment_uri,
        0x5c,
    )
    .await;
    write_library_bytes(
        &app,
        &token,
        &format!("{malformed_segment_uri}/segments/00000000.m4s"),
        b"not a valid segment",
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &malformed_segment_uri,
        invalid_message,
    )
    .await;

    let unknown_track_uri = format!("{root}/Documents/protected-unknown-track");
    create_runtime_custody_publish_directory(dir.path(), &app, &token, &unknown_track_uri, 0x5d)
        .await;
    let unknown_track_segment = make_clear_segment(99, b"badsegxx");
    write_library_bytes(
        &app,
        &token,
        &format!("{unknown_track_uri}/segments/00000000.m4s"),
        &unknown_track_segment,
    )
    .await;
    assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &unknown_track_uri,
        invalid_message,
    )
    .await;
}

#[tokio::test]
async fn test_library_provider_runtime_custody_publish_passes_one_file_to_runtime_media_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state_without_content(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/source-video.mp4");
    write_library_bytes(&app, &token, &uri, b"source media bytes").await;

    let payload = assert_runtime_custody_publish_error(
        dir.path(),
        &app,
        &token,
        &authority.principal_id,
        &uri,
        "Runtime custody media preparation provider is unavailable",
    )
    .await;
    let text = payload.to_string();
    assert!(!text.contains("source-video.mp4"));
    assert!(!text.contains("input.bin"));
    assert!(!text.contains("staging"));
}

#[tokio::test]
async fn test_library_provider_unpublish_and_repair_update_status() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{root}/Documents/availability.txt");

    let (write_status, _) = post_library(
        app.clone(),
        &token,
        "write",
        json!({
            "uri": uri,
            "data": base64::engine::general_purpose::STANDARD.encode(b"availability"),
        }),
    )
    .await;
    assert_eq!(write_status, StatusCode::OK);

    let (publish_status, publish) = post_library(
        app.clone(),
        &token,
        "publish",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(publish_status, StatusCode::OK);
    let revision = publish["data"]["object"]["revision"].as_str().unwrap();

    let (unpublish_status, unpublish) = post_library(
        app.clone(),
        &token,
        "unpublish",
        json!({
            "uri": uri,
            "if_revision": revision,
        }),
    )
    .await;
    assert_eq!(unpublish_status, StatusCode::OK);
    assert_eq!(unpublish["status"], "ok");
    assert_eq!(unpublish["data"]["object"]["published"], false);
    assert!(unpublish["data"]["object"]["content_cid"]
        .as_str()
        .unwrap()
        .starts_with("bafkrei"));
    assert_eq!(unpublish["data"]["object"].get("published_cid"), None);
    assert_eq!(
        unpublish["data"]["object"]["availability"],
        "local_unpinned"
    );

    let (repair_status, repair) = post_library(
        app.clone(),
        &token,
        "repair",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(repair_status, StatusCode::OK);
    assert_eq!(repair["status"], "ok");
    assert_eq!(repair["data"]["object"]["published"], true);
    assert_eq!(repair["data"]["object"]["availability"], "local_pinned");

    let (status_code, status) = post_library(
        app,
        &token,
        "status",
        json!({
            "uri": uri,
        }),
    )
    .await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(status["data"]["object"]["published"], true);
    assert_eq!(
        status["data"]["published"]["availability"]["status"],
        "local_pinned"
    );
}
