//! Opt-in diagnostic authority for the installed, two-Home model fixture.
//! This prepares persistent inputs only. The operator owns installation, port
//! checks, process startup, HTTP proof, and retention of both test Homes.

use super::home_system::write_home_principal_object_json_for_authority;
use super::*;
use crate::collaboration_contact_store::CollaborationContactStore;
use crate::collaboration_default_conversation::*;
use crate::collaboration_discovery::*;
use crate::collaboration_network::*;
use crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument;
use crate::collaboration_startup::*;
use elastos_common::collaboration_protocol::*;
use elastos_runtime::signature::{generate_keypair, SigningKey};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

const ROOT_NAME: &str = "elastos-codex-model-negative-fixture";
const MARKER: &str = ".model-installed-fixture-v1";
const MARKER_BYTES: &[u8] = b"Test-owned diagnostic model authority. Retain for operator review.\n";
const NETWORK: &str = "model-installed-fixture";
const REQUEST_ID: &str = "model-installed-fixture-request";

fn write_new_private(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("fixture output must be a new file");
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn marked_directory(path: &Path) {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        let marker = path.join(MARKER);
        assert!(std::fs::symlink_metadata(&marker).unwrap().is_file());
        assert_eq!(std::fs::read(marker).unwrap(), MARKER_BYTES);
        assert_eq!(metadata.permissions().mode() & 0o077, 0);
    } else {
        std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        write_new_private(&path.join(MARKER), MARKER_BYTES);
    }
}

fn fixture_root(require_new: bool) -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("MODEL_INSTALLED_FIXTURE_ROOT")
            .expect("set MODEL_INSTALLED_FIXTURE_ROOT to the explicit test-owned root"),
    );
    assert!(root.is_absolute());
    assert_eq!(
        root.file_name().and_then(|name| name.to_str()),
        Some(ROOT_NAME)
    );
    assert!(root
        .components()
        .all(|part| matches!(part, Component::RootDir | Component::Normal(_))));
    // Require an existing real parent; refuse aliases into another Home.
    for ancestor in root.ancestors().skip(1) {
        let metadata = std::fs::symlink_metadata(ancestor).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    }
    marked_directory(&root);
    if require_new {
        assert!(
            !root.join("diagnostic-authority.private.json").exists(),
            "fixture already provisioned; preserve it for review"
        );
        for role in ["owner", "consumer"] {
            let home = root.join(role);
            if home.exists() {
                marked_directory(&home);
                assert!(
                    !home.join("Library").exists(),
                    "fixture Home already contains data; preserve it for review"
                );
            }
        }
    }
    root
}

fn new_home(root: &Path, role: &str) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let home = root.join(role);
    marked_directory(&home);
    let mut data = home;
    for component in ["Library", "Application Support", "elastos"] {
        data.push(component);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&data)
            .unwrap();
    }
    data
}

fn peer(key: &SigningKey, port: u16) -> CollaborationBootstrapPeer {
    let id = iroh::SecretKey::from_bytes(&key.to_bytes()).public();
    let endpoint = iroh::EndpointAddr::from(id)
        .with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
    let bytes = serde_json::to_vec(&json!({ "topic": null, "endpoints": [endpoint] })).unwrap();
    let peer = CollaborationBootstrapPeer {
        node_id: id.to_string(),
        connect_ticket: data_encoding::BASE32_NOPAD
            .encode(&bytes)
            .to_ascii_lowercase(),
    };
    validate_collaboration_bootstrap_peer(&peer).unwrap();
    peer
}

fn network_inputs(
    peers: Vec<CollaborationBootstrapPeer>,
) -> (Vec<u8>, Vec<u8>, VerifiedCollaborationNetworkProfile) {
    let (key, _) = generate_keypair();
    let signer_did = crate::crypto::encode_signing_key_did(&key);
    // The default grant starts the installed discovery service, which projects
    // accepted Profile contacts for the consumer's Services authority.
    let grant = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
        schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
        network_id: NETWORK.to_string(),
        conversation_id: "model-fixture-room".to_string(),
        sender_service: crate::collaboration_product::CHAT_SERVICE.to_string(),
        admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
    })
    .unwrap();
    let payload = CollaborationNetworkProfile {
        schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
        network_id: NETWORK.to_string(),
        revision: 1,
        previous_profile_sha256: None,
        signer_did: signer_did.clone(),
        bootstrap_peers: peers,
        default_conversation: Some(DefaultConversationGrantDescriptor {
            grant_cid: raw_sha256_cid(&grant).unwrap(),
        }),
    };
    let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
        &key,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
        &canonical_collaboration_network_profile_payload_bytes(&payload).unwrap(),
    );
    let envelope = serde_json::to_vec(
        &serde_json::to_value(SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did: envelope_signer,
        })
        .unwrap(),
    )
    .unwrap();
    let CollaborationNetworkProfileMode::Configured(network) =
        validate_collaboration_network_profile(
            Some(&envelope),
            NETWORK,
            std::slice::from_ref(&signer_did),
            None,
        )
        .unwrap()
    else {
        panic!("configured fixture network required")
    };
    let config = canonical_startup_config_bytes(&CollaborationStartupConfigFile {
        schema: COLLABORATION_STARTUP_CONFIG_SCHEMA.to_string(),
        expected_network_id: NETWORK.to_string(),
        trusted_profile_signer_dids: vec![signer_did],
        profile_chain_base64: vec![base64::engine::general_purpose::STANDARD.encode(&envelope)],
        default_conversation_grant_base64: Some(
            base64::engine::general_purpose::STANDARD.encode(grant),
        ),
    })
    .unwrap();
    parse_and_validate_collaboration_startup_configuration(&config).unwrap();
    (config, envelope, network)
}

fn signed_message(
    key: &SigningKey,
    profile: &VerifiedCollaborationProfileDocument,
    conversation: &str,
    recipient: CollaborationRecipient,
    payload_type: &str,
    payload: Value,
    validity: std::ops::Range<u64>,
) -> Vec<u8> {
    let payload = CollaborationMessage {
        schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
        network_id: NETWORK.to_string(),
        conversation_id: conversation.to_string(),
        message_id: crate::collaboration_core::random_hex_128().unwrap(),
        nonce: crate::collaboration_core::random_hex_128().unwrap(),
        created_at: validity.start,
        expires_at: validity.end,
        sender_profile_did: profile.document().profile_did.clone(),
        sender_service: COLLABORATION_DISCOVERY_SERVICE.to_string(),
        recipient,
        payload_type: payload_type.to_string(),
        payload,
    };
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        key,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        &canonical_collaboration_message_bytes(&payload).unwrap(),
    );
    canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
        payload,
        signature,
        signer_did,
    })
    .unwrap()
}

fn accept_contact(
    store: &CollaborationContactStore,
    local_key: &SigningKey,
    local: &VerifiedCollaborationProfileDocument,
    remote_key: &SigningKey,
    remote: &VerifiedCollaborationProfileDocument,
) {
    let now = now_ts().saturating_sub(3);
    let advertisement = signed_message(
        local_key,
        local,
        COLLABORATION_DISCOVERY_DIRECTORY_ID,
        CollaborationRecipient {
            kind: CollaborationRecipientKind::Conversation,
            id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
        },
        COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        serde_json::to_value(CollaborationDiscoveryAdvertisementPayload {
            signed_profile: local.signed_envelope().clone(),
        })
        .unwrap(),
        now..now + COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
    );
    store
        .store_local_advertisement(&advertisement, now)
        .unwrap();
    let request = signed_message(
        remote_key,
        remote,
        COLLABORATION_DISCOVERY_CONTACT_ID,
        CollaborationRecipient {
            kind: CollaborationRecipientKind::Profile,
            id: local.document().profile_did.clone(),
        },
        COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
        serde_json::to_value(CollaborationContactRequestPayload {
            advertisement_envelope_sha256: collaboration_message_envelope_sha256(&advertisement),
            signed_profile: remote.signed_envelope().clone(),
        })
        .unwrap(),
        now + 1..now + 1 + COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS,
    );
    store
        .record_incoming_contact_request(&request, now + 1)
        .unwrap();
    let request_envelope: SignedCollaborationMessage = serde_json::from_slice(&request).unwrap();
    let payload = CollaborationContactDecisionReceipt {
        schema: COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1.to_string(),
        network_id: NETWORK.to_string(),
        request_envelope_sha256: collaboration_message_envelope_sha256(&request),
        conversation_id: COLLABORATION_DISCOVERY_CONTACT_ID.to_string(),
        requester_profile_did: remote.document().profile_did.clone(),
        requester_endpoint_did: request_envelope.signer_did,
        request_message_id: request_envelope.payload.message_id,
        request_message_nonce: request_envelope.payload.nonce,
        recipient_profile_did: local.document().profile_did.clone(),
        recipient_endpoint_did: crate::crypto::encode_signing_key_did(local_key),
        decision: CollaborationContactDecision::Accepted,
        decided_at: now + 2,
    };
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        local_key,
        COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
        &serde_json::to_vec(&serde_json::to_value(&payload).unwrap()).unwrap(),
    );
    let receipt = canonical_signed_collaboration_contact_decision_receipt_bytes(
        &SignedCollaborationContactDecisionReceipt {
            payload,
            signature,
            signer_did,
        },
    )
    .unwrap();
    store
        .record_contact_decision_receipt(&receipt, now + 2)
        .unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.contacts().len(), 1);
    assert_eq!(
        snapshot.contacts()[0].remote_profile_did(),
        remote.document().profile_did
    );
    assert_eq!(
        snapshot.contacts()[0].remote_presence_device_did(),
        crate::crypto::encode_signing_key_did(remote_key)
    );
}

fn principal_output(data: &Path, authority: &TestPasskeyAuthority) -> Value {
    let mut tokens = serde_json::Map::new();
    for app in ["system", "assistant", "marketplace", "services"] {
        tokens.insert(
            app.to_string(),
            json!(app_token_for_authority(data, app, authority)),
        );
    }
    json!({
        "principal_id": authority.principal_id,
        "profile_did": load_profile_for_authority(data, authority).document().profile_did,
        "localhost_root": crate::auth::principal_localhost_root(&authority.principal_id),
        "tokens": tokens,
    })
}

#[test]
#[ignore = "explicit MODEL_INSTALLED_FIXTURE_ROOT; creates retained diagnostic authority for installed proof"]
fn provision_model_installed_fixture() {
    use crate::api::gateway::gateway_home_runtime::home_people_contact_id;
    use crate::api::gateway::gateway_home_system::home_services_contact_offer_id;
    use crate::api::gateway::gateway_model_service::{
        model_grant_id, MODEL_GRANT_SCHEMA, MODEL_GRANT_TTL_SECS, MODEL_OPERATIONS,
    };
    let root = fixture_root(true);
    let owner_data = new_home(&root, "owner");
    let consumer_data = new_home(&root, "consumer");
    for (data, port) in [(&owner_data, 61972), (&consumer_data, 61973)] {
        write_new_private(
            &data.join("config.toml"),
            format!("carrier_bind_addr = \"127.0.0.1:{port}\"\n").as_bytes(),
        );
    }
    let owner = passkey_authority_with_profile_role_credential(
        &owner_data,
        "Model fixture owner",
        crate::auth::RuntimePrincipalRole::Admin,
        "model-installed-owner-v1",
    );
    let consumer = passkey_authority_with_profile_role_credential(
        &consumer_data,
        "Model fixture consumer",
        crate::auth::RuntimePrincipalRole::Admin,
        "model-installed-consumer-v1",
    );
    let wrong = passkey_authority_with_profile_role_credential(
        &consumer_data,
        "Model fixture other principal",
        crate::auth::RuntimePrincipalRole::Admin,
        "model-installed-other-v1",
    );
    assert_ne!(consumer.principal_id, wrong.principal_id);
    assert_ne!(owner.principal_id, consumer.principal_id);
    let (owner_key, owner_did) = elastos_identity::load_or_create_did(&owner_data).unwrap();
    let (consumer_key, consumer_did) =
        elastos_identity::load_or_create_did(&consumer_data).unwrap();
    let owner_peer = peer(&owner_key, 61972);
    let consumer_peer = peer(&consumer_key, 61973);
    let (config, envelope, network) = network_inputs(vec![owner_peer.clone()]);
    for data in [&owner_data, &consumer_data] {
        write_new_private(&data.join(COLLABORATION_STARTUP_CONFIG_FILE), &config);
    }
    write_new_private(&root.join("network-profile.signed.json"), &envelope);
    let owner_profile = load_profile_for_authority(&owner_data, &owner);
    let consumer_profile = load_profile_for_authority(&consumer_data, &consumer);
    for (data, authority, local_key, local_did, local_profile, remote_key, remote_profile) in [
        (
            &owner_data,
            &owner,
            &owner_key,
            &owner_did,
            &owner_profile,
            &consumer_key,
            &consumer_profile,
        ),
        (
            &consumer_data,
            &consumer,
            &consumer_key,
            &consumer_did,
            &consumer_profile,
            &owner_key,
            &owner_profile,
        ),
    ] {
        let store = CollaborationContactStore::new(
            data,
            &authority.principal_id,
            &crate::auth::principal_localhost_root(&authority.principal_id),
            network.clone(),
            local_profile,
            local_did,
        )
        .unwrap();
        accept_contact(&store, local_key, local_profile, remote_key, remote_profile);
    }
    let now = now_ts();
    let expiry = now + MODEL_GRANT_TTL_SECS;
    let grant_id = model_grant_id(REQUEST_ID);
    let remote_offer = home_services_contact_offer_id(
        &home_people_contact_id(&owner_profile.document().profile_did),
        "remote_model",
    );
    write_home_principal_object_json_for_authority(
        &owner_data,
        &owner,
        "services-requests.json",
        json!({
            "schema": "elastos.services.requests/v1", "principal_id": owner.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&owner.principal_id), "updated_at": now,
            "requests": { REQUEST_ID: {
                "request_id": REQUEST_ID, "offer_id": "local:provider:model", "service_uri": "elastos://peer/model",
                "service_kind": "remote_model", "service_display_name": "Model diagnostic fixture",
                "requester_peer_id": consumer_peer.node_id, "requester_did": consumer_did,
                "requester_principal_id": consumer.principal_id, "requester_display_name": "Model fixture consumer",
                "created_at": now, "updated_at": now, "status": "approved", "authenticated_request": true,
                "grant_expires_at": expiry,
            } },
        }),
    );
    write_home_principal_object_json_for_authority(
        &consumer_data,
        &consumer,
        "services-state.json",
        json!({
            "schema": "elastos.services.state/v1", "principal_id": consumer.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&consumer.principal_id), "updated_at": now,
            "local_offer_ids": [], "remote_offer_ids": [remote_offer],
            "remote_offer_requests": { remote_offer.clone(): {
                "request_id": REQUEST_ID, "offer_id": remote_offer, "service_uri": "elastos://peer/model",
                "service_kind": "remote_model", "service_display_name": "Model diagnostic fixture",
                "target_peer_id": owner_peer.node_id, "created_at": now, "updated_at": now, "status": "approved",
                "remote_model_grant": {
                    "schema": MODEL_GRANT_SCHEMA, "id": "remote-model-installed-fixture", "grant_id": grant_id,
                    "peer_did": owner_peer.node_id, "connect_ticket": owner_peer.connect_ticket,
                    "principal_id": consumer.principal_id, "service_display_name": "Model diagnostic fixture",
                    "offer_scope": "local_engines", "operations": MODEL_OPERATIONS, "expires_at": expiry,
                },
            } },
        }),
    );
    let private = json!({
        "schema": "elastos.model.installed-diagnostic-fixture/v1",
        "authority": "diagnostic test authority; ordinary UI acceptance requires separate evidence",
        "created_at": now, "grant_expires_at": expiry, "request_id": REQUEST_ID, "grant_id": grant_id,
        "network_id": NETWORK, "carrier_network": "direct", "dummy_provider_key": "sk-or-fixture-valid",
        "owner": { "home": root.join("owner"), "data": owner_data, "gateway_port": 61970,
            "carrier_port": 61972, "device_did": owner_did, "peer_id": owner_peer.node_id,
            "authority": principal_output(&owner_data, &owner) },
        "consumer": { "home": root.join("consumer"), "data": consumer_data, "gateway_port": 61971,
            "carrier_port": 61973, "device_did": consumer_did, "peer_id": consumer_peer.node_id,
            "authority": principal_output(&consumer_data, &consumer),
            "wrong_principal": principal_output(&consumer_data, &wrong) },
    });
    let output = root.join("diagnostic-authority.private.json");
    write_new_private(&output, &serde_json::to_vec_pretty(&private).unwrap());
    println!("Diagnostic model fixture prepared at {}. Private authority file: {}. Owner gateway 61970 / Carrier 61972; consumer gateway 61971 / Carrier 61973. Grant {} expires at {}. Retain both Homes for operator review.", root.display(), output.display(), grant_id, expiry);
}

/// Renew only the marked fixture's Assistant launch authority after its
/// short-lived token expires. The prior private manifest stays in an explicit
/// operator backup; human Home sessions are outside this test root.
#[test]
#[ignore = "explicit marked fixture root, action and private backup path"]
fn refresh_model_installed_fixture_assistant_token() {
    assert_eq!(
        std::env::var("MODEL_INSTALLED_FIXTURE_ACTION").unwrap(),
        "refresh-consumer-assistant-token"
    );
    let root = fixture_root(false);
    let output = root.join("diagnostic-authority.private.json");
    let before = std::fs::read(&output).unwrap();
    let backup = PathBuf::from(std::env::var_os("MODEL_INSTALLED_FIXTURE_TOKEN_BACKUP").unwrap());
    assert!(backup.is_absolute() && !backup.exists());
    write_new_private(&backup, &before);
    let mut manifest: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        manifest["schema"],
        "elastos.model.installed-diagnostic-fixture/v1"
    );
    let data = root.join("consumer/Library/Application Support/elastos");
    assert_eq!(manifest["consumer"]["data"].as_str(), data.to_str());
    let principal_id = manifest["consumer"]["authority"]["principal_id"]
        .as_str()
        .unwrap();
    let principal = crate::auth::active_passkey_principals(&data)
        .unwrap()
        .into_iter()
        .find(|principal| {
            principal.principal_id == principal_id && crate::auth::is_admin(principal)
        })
        .unwrap();
    let now = crate::auth::now_ts();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", uuid_like_token()),
        session_id: format!("auth:{}", uuid_like_token()),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec!["assistant".to_string()],
    };
    crate::auth::store_session_grant(&data, grant.clone()).unwrap();
    let token = issue_home_launch_token_for_auth_grant(&data, "assistant", &grant).unwrap();
    manifest["consumer"]["authority"]["tokens"]["assistant"] = json!(token);
    let staged = root.join(format!(
        ".diagnostic-authority.{}.private.json",
        uuid_like_token()
    ));
    write_new_private(&staged, &serde_json::to_vec_pretty(&manifest).unwrap());
    std::fs::rename(&staged, &output).unwrap();
    std::fs::File::open(&root).unwrap().sync_all().unwrap();
    println!(
        "Refreshed only the marked Consumer Assistant diagnostic token; prior manifest preserved."
    );
}

/// Explicit fixture time/authority control; never used by ordinary Home UI.
#[test]
#[ignore = "explicit marked fixture root and MODEL_INSTALLED_FIXTURE_ACTION"]
fn change_model_installed_fixture_authority() {
    let action = std::env::var("MODEL_INSTALLED_FIXTURE_ACTION").unwrap();
    assert!(matches!(
        action.as_str(),
        "expire" | "revoke" | "wrong-principal" | "unapproved" | "restore"
    ));
    let root = fixture_root(false);
    let private: Value = serde_json::from_slice(
        &std::fs::read(root.join("diagnostic-authority.private.json")).unwrap(),
    )
    .unwrap();
    let data = root.join("owner/Library/Application Support/elastos");
    assert_eq!(private["owner"]["data"].as_str(), data.to_str());
    let principal = private["owner"]["authority"]["principal_id"]
        .as_str()
        .unwrap();
    let localhost = crate::auth::principal_localhost_root(principal);
    let uri = format!("{localhost}/.AppData/ElastOS/Home/services-requests.json");
    let path = elastos_common::localhost::rooted_localhost_fs_path(&data, &uri).unwrap();
    let bytes =
        crate::auth::read_principal_root_object(&data, principal, &localhost, &uri, &path).unwrap();
    let mut record: Value = serde_json::from_slice(&bytes).unwrap();
    let request = record["requests"].get_mut(REQUEST_ID).unwrap();
    assert_eq!(request["request_id"], REQUEST_ID);
    request["updated_at"] = json!(now_ts());
    request["requester_principal_id"] = json!(if action == "wrong-principal" {
        private["consumer"]["wrong_principal"]["principal_id"]
            .as_str()
            .unwrap()
    } else {
        private["consumer"]["authority"]["principal_id"]
            .as_str()
            .unwrap()
    });
    request["status"] = json!(match action.as_str() {
        "revoke" => "denied",
        "unapproved" => "pending",
        _ => "approved",
    });
    request["grant_expires_at"] = json!(if action == "expire" {
        now_ts() - 1
    } else {
        private["grant_expires_at"].as_u64().unwrap()
    });
    crate::auth::write_principal_root_object(
        &data,
        principal,
        &localhost,
        &uri,
        &path,
        &serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    println!("Applied {action} to the marked diagnostic fixture grant only.");
}
