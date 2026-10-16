use super::super::home_system::{
    configured_discovery_network_profile_for_test, home_test_post_json,
    signed_discovery_message_for_test, TestCollaborationMessageScope,
};
use super::*;
use elastos_runtime::signature::{generate_keypair, SigningKey};

const NETWORK: &str = "gateway-chat-contact-request";
const CONTACT_REQUEST_PATH: &str = "/api/apps/chat-room/contacts/request";

/// A Home with a Profile, a Discovery service and a remote Profile whose
/// signed advertisement the tests can make visible. No relay is reachable.
struct ContactRequestHome {
    dir: tempfile::TempDir,
    app: Router,
    authority: TestPasskeyAuthority,
    service: crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    store: crate::collaboration_contact_store::CollaborationContactStore,
    local_profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    remote_profile_did: String,
    remote_advertisement: Vec<u8>,
    chat_context: HomeLaunchTokenContext,
    chat_token: String,
    now: u64,
}

impl ContactRequestHome {
    async fn new() -> Self {
        let now = crate::auth::now_ts();
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let authority = passkey_authority_with_name(dir.path(), Some("Local"));
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);

        let (trusted_key, _) = generate_keypair();
        let network_profile = configured_discovery_network_profile_for_test(&trusted_key, NETWORK);
        // Production keys Discovery with the runtime device identity, which is
        // the device the local Profile authorizes.
        let (runtime_device_key, _) = elastos_identity::load_or_create_did(dir.path()).unwrap();
        let service = crate::collaboration_discovery_runtime::CollaborationDiscoveryService::new(
            SigningKey::from_bytes(&runtime_device_key.to_bytes()),
            network_profile.clone(),
            Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
        )
        .await
        .unwrap();
        let mut state = test_state(dir.path());
        state.collaboration_discovery_service = Some(service.clone());
        let app = gateway_router(state);

        let (status, _) = home_test_post_json(
            &app,
            "/api/apps/people/profile",
            &authority.people_token,
            "null",
            serde_json::json!({ "display_name": "Local Profile" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
        let local_profile = crate::collaboration_profile_authority::load_profile_authority(
            dir.path(),
            &authority.principal_id,
            &localhost_root,
        )
        .unwrap()
        .unwrap();
        let local_device_did =
            crate::collaboration_profile_authority::load_existing_device_did(dir.path())
                .unwrap()
                .unwrap();
        let store = crate::collaboration_contact_store::CollaborationContactStore::new(
            dir.path(),
            &authority.principal_id,
            &localhost_root,
            network_profile,
            &local_profile,
            &local_device_did,
        )
        .unwrap();

        let (remote_profile_did, remote_advertisement) = remote_advertisement("Remote", now);
        let chat_context = HomeLaunchTokenContext {
            principal_id: authority.principal_id.clone(),
            session_id: authority.session_id.clone(),
            proof_binding_id: Some(authority.proof_binding_id.clone()),
            grant_id: authority.grant_id.clone(),
        };
        let chat_token = issue_home_projection_launch_token_with_context(
            dir.path(),
            CHAT_ROOM_CAPSULE_ID,
            CHAT_ROOM_CAPSULE_ID,
            &chat_context,
        )
        .unwrap();
        Self {
            dir,
            app,
            authority,
            service,
            store,
            local_profile,
            remote_profile_did,
            remote_advertisement,
            chat_context,
            chat_token,
            now,
        }
    }

    fn show(&self, visible: Vec<Vec<u8>>) {
        self.service
            .seed_visible_discovery_for_test(&self.store, &self.local_profile, visible, self.now)
            .unwrap();
    }

    async fn request(&self, token: Option<&str>, participant_ref: &str) -> (StatusCode, String) {
        self.post(
            CONTACT_REQUEST_PATH,
            token,
            serde_json::json!({ "participant_ref": participant_ref }),
        )
        .await
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        payload: serde_json::Value,
    ) -> (StatusCode, String) {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header(HOST, "localhost:61180")
            .header("origin", "null")
            .header(CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header("x-elastos-home-token", token);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(payload.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn outgoing_requests(&self) -> usize {
        self.store
            .outgoing_pending_requests(self.now)
            .unwrap()
            .len()
    }

    fn incoming_request(&self) -> String {
        let advertisement = self
            .store
            .published_local_advertisement(self.now)
            .unwrap()
            .unwrap();
        let (key, _) = generate_keypair();
        let key = SigningKey::from_bytes(&key.to_bytes());
        let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
            &key,
            "Requester",
            None,
            1,
            None,
            self.now,
            vec![crate::crypto::encode_signing_key_did(&key)],
        )
        .unwrap();
        let request = signed_discovery_message_for_test(
            &key,
            &profile.document().profile_did,
            TestCollaborationMessageScope {
                network_id: NETWORK,
                conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_ID,
            },
            elastos_common::collaboration_protocol::CollaborationRecipient {
                kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Profile,
                id: self.local_profile.document().profile_did.clone(),
            },
            crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            serde_json::to_value(crate::collaboration_discovery::CollaborationContactRequestPayload {
                advertisement_envelope_sha256:
                    elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(
                        &advertisement,
                    ),
                signed_profile: profile.signed_envelope().clone(),
            })
            .unwrap(),
            self.now..self.now
                + crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS,
        );
        self.store
            .record_incoming_contact_request(&request, self.now)
            .unwrap();
        elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(&request)
    }

    fn contact_bytes(&self) -> Vec<u8> {
        let uri = format!(
            "{}/.AppData/ElastOS/People/contact-state.json",
            self.store.localhost_root()
        );
        let path =
            elastos_common::localhost::rooted_localhost_fs_path(self.dir.path(), &uri).unwrap();
        std::fs::read(path).unwrap()
    }
}

/// A remote person's signed Discovery advertisement and Profile DID.
fn remote_advertisement(display_name: &str, now: u64) -> (String, Vec<u8>) {
    let (device_key, _) = generate_keypair();
    let device_key = SigningKey::from_bytes(&device_key.to_bytes());
    let device_did = crate::crypto::encode_signing_key_did(&device_key);
    let (profile_key, _) = generate_keypair();
    let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
        &SigningKey::from_bytes(&profile_key.to_bytes()),
        display_name,
        None,
        1,
        None,
        now,
        vec![device_did],
    )
    .unwrap();
    let profile_did = profile.document().profile_did.clone();
    let advertisement = signed_discovery_message_for_test(
        &device_key,
        &profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Conversation,
            id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationDiscoveryAdvertisementPayload {
                signed_profile: profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        now..now + crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
    );
    (profile_did, advertisement)
}

#[tokio::test]
async fn test_chat_contact_request_refuses_callers_without_a_chat_session() {
    let home = ContactRequestHome::new().await;
    home.show(vec![home.remote_advertisement.clone()]);
    let participant_ref = home_people_contact_id(&home.remote_profile_did);

    let (status, body) = home.request(None, &participant_ref).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let expired = issue_expired_home_launch_token_with_context(
        home.dir.path(),
        CHAT_ROOM_CAPSULE_ID,
        &home.chat_context,
    )
    .unwrap();
    let (status, body) = home.request(Some(&expired), &participant_ref).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A valid launch token for another app passes the front door, then Chat
    // refuses it.
    let (status, body) = home
        .request(Some(&home.authority.people_token), &participant_ref)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    assert_eq!(home.outgoing_requests(), 0);
}

#[tokio::test]
async fn test_chat_contact_request_refuses_a_person_not_visible_in_discovery() {
    let home = ContactRequestHome::new().await;
    let participant_ref = home_people_contact_id(&home.remote_profile_did);

    // Nobody is visible yet.
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("turn on Discovery"), "{body}");

    // Someone else is visible, not this person.
    let (_, other_advertisement) = remote_advertisement("Other", home.now);
    home.show(vec![other_advertisement]);
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // The person is visible, but this Home turned Discovery off.
    home.show(vec![home.remote_advertisement.clone()]);
    home.store.set_discovery_enabled(false, home.now).unwrap();
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(!body.contains("did:key:"), "{body}");

    assert_eq!(home.outgoing_requests(), 0);
}

#[tokio::test]
async fn test_chat_contact_request_queues_one_request_for_a_visible_person() {
    let home = ContactRequestHome::new().await;
    home.show(vec![home.remote_advertisement.clone()]);
    let participant_ref = home_people_contact_id(&home.remote_profile_did);

    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({ "status": "requested" })
    );
    assert!(!body.contains("did:key:"), "{body}");
    assert_eq!(home.outgoing_requests(), 1);

    // Asking again reuses the stored request.
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(home.outgoing_requests(), 1);
}

#[tokio::test]
async fn left_home_refuses_new_contact_intent_and_both_inbox_decisions_without_mutation() {
    let mut home = ContactRequestHome::new().await;
    let membership = Arc::new(
        crate::collaboration_release_network::CommunityMembership::load(home.dir.path(), NETWORK)
            .unwrap(),
    );
    home.service = home
        .service
        .clone()
        .with_community_membership(membership.clone());
    let mut state = test_state(home.dir.path());
    state.collaboration_discovery_service = Some(home.service.clone());
    home.app = gateway_router(state);
    home.show(vec![home.remote_advertisement.clone()]);
    let participant_ref = home_people_contact_id(&home.remote_profile_did);
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(home.outgoing_requests(), 1);
    let incoming = home.incoming_request();
    let inbox_token = issue_home_projection_launch_token_with_context(
        home.dir.path(),
        INBOX_CAPSULE_ID,
        INBOX_CAPSULE_ID,
        &home.chat_context,
    )
    .unwrap();
    let before = home.contact_bytes();
    membership.set_joined(false).unwrap();
    let expected = crate::collaboration_release_network::COMMUNITY_LEFT_DETAIL;

    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, expected);
    assert_eq!(home.contact_bytes(), before);

    let advertisement_id =
        elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(
            &home.remote_advertisement,
        );
    let (status, body) = home
        .post(
            "/api/apps/people/discovery/requests",
            Some(&home.authority.people_token),
            serde_json::json!({ "advertisement_id": advertisement_id }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, expected);
    assert_eq!(home.contact_bytes(), before);

    for prefix in ["contact-accept-request:", "contact-decline-request:"] {
        let (status, body) = home
            .post(
                "/api/apps/inbox/actions",
                Some(&inbox_token),
                serde_json::json!({ "action_id": format!("{prefix}{incoming}") }),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body, expected);
        assert_eq!(home.contact_bytes(), before);
        assert_eq!(home.store.pending_incoming_requests().unwrap().len(), 1);
        assert!(home
            .store
            .stored_contact_decision_receipt(&incoming)
            .unwrap()
            .is_none());
    }
    assert!(home.store.discovery_enabled().unwrap());
    assert_eq!(home.outgoing_requests(), 1);

    membership.set_joined(true).unwrap();
    let (status, body) = home
        .post(
            "/api/apps/inbox/actions",
            Some(&inbox_token),
            serde_json::json!({ "action_id": format!("contact-accept-request:{incoming}") }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(home.store.pending_incoming_requests().unwrap().is_empty());
    assert!(home
        .store
        .stored_contact_decision_receipt(&incoming)
        .unwrap()
        .is_some());
    assert_eq!(home.store.snapshot().unwrap().contacts().len(), 1);
    assert_eq!(home.outgoing_requests(), 1);
}
