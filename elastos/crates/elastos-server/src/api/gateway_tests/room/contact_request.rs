use super::super::home_system::{
    configured_discovery_network_profile_for_test, home_test_get_json, home_test_post_json,
    signed_discovery_message_for_test, TestCollaborationMessageScope,
};
use super::*;
use elastos_runtime::signature::{generate_keypair, SigningKey};

const NETWORK: &str = "gateway-chat-contact-request";
const CONVERSATION: &str = "community";
const CONTACT_REQUEST_PATH: &str = "/api/apps/chat-room/contacts/request";
const PROFILE_ID_PATH: &str = "/api/apps/chat-room/contacts/profile-id";
const PEOPLE_REQUEST_PATH: &str = "/api/apps/people/contacts/request";
const NOT_LISTED: &str = "This person is no longer listed in Community.";

/// A Home with a Profile, a Discovery service and configured Chat, in whose
/// Community a remote person has posted, so Chat lists them. Their signed
/// Discovery advertisement can be made visible. No relay is reachable.
struct ContactRequestHome {
    dir: tempfile::TempDir,
    app: Router,
    authority: TestPasskeyAuthority,
    service: crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    chat: crate::collaboration_product::CollaborationChatProductPort,
    store: crate::collaboration_contact_store::CollaborationContactStore,
    local_profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    remote: RemotePerson,
    chat_context: HomeLaunchTokenContext,
    chat_token: String,
    now: u64,
}

/// A person on another Home. Their Profile authorizes that Home's Chat
/// device and a second device that signs their Discovery advertisement.
struct RemotePerson {
    dir: tempfile::TempDir,
    chat: crate::collaboration_product::CollaborationChatProductPort,
    profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    advertisement: Vec<u8>,
}

impl RemotePerson {
    fn new(display_name: &str, now: u64) -> Self {
        Self::declaring(display_name, now, Vec::new())
    }

    /// A person whose signed Profile also lists `declared` DIDs as its
    /// devices, which any Profile may do without holding their keys.
    fn declaring(display_name: &str, now: u64, declared: Vec<String>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let chat =
            crate::collaboration_product::test_chat_product_port(dir.path(), NETWORK, CONVERSATION);
        let (device_key, _) = generate_keypair();
        let device_key = SigningKey::from_bytes(&device_key.to_bytes());
        let (profile_key, _) = generate_keypair();
        let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
            &SigningKey::from_bytes(&profile_key.to_bytes()),
            display_name,
            None,
            1,
            None,
            now,
            [
                chat.test_core().test_local_device_did(),
                crate::crypto::encode_signing_key_did(&device_key),
            ]
            .into_iter()
            .chain(declared)
            .collect(),
        )
        .unwrap();
        let advertisement = signed_discovery_message_for_test(
            &device_key,
            &profile.document().profile_did,
            TestCollaborationMessageScope {
                network_id: NETWORK,
                conversation_id:
                    crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID,
            },
            elastos_common::collaboration_protocol::CollaborationRecipient {
                kind:
                    elastos_common::collaboration_protocol::CollaborationRecipientKind::Conversation,
                id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID
                    .to_string(),
            },
            crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            serde_json::to_value(
                crate::collaboration_discovery::CollaborationDiscoveryAdvertisementPayload {
                    signed_profile: profile.signed_envelope().clone(),
                },
            )
            .unwrap(),
            now..now
                + crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
        );
        Self {
            dir,
            chat,
            profile,
            advertisement,
        }
    }

    fn did(&self) -> &str {
        &self.profile.document().profile_did
    }

    fn participant_ref(&self) -> String {
        home_people_contact_id(self.did())
    }
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
        let chat =
            crate::collaboration_product::test_chat_product_port(dir.path(), NETWORK, CONVERSATION);
        let app = contact_request_router(dir.path(), &service, &chat);

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

        let remote = RemotePerson::new("Remote", now);
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
        let home = Self {
            dir,
            app,
            authority,
            service,
            chat,
            store,
            local_profile,
            remote,
            chat_context,
            chat_token,
            now,
        };
        home.post_in_community(&home.remote);
        home
    }

    /// `remote` posts in this Home's Community: their Home signs a message
    /// that this Home ingests and projects, so Chat lists them as verified.
    fn post_in_community(&self, remote: &RemotePerson) {
        let body = format!("hello from {}", remote.profile.document().display_name);
        let prepared = remote
            .chat
            .prepare_message(
                crate::collaboration_product::chat_message_request_binding(
                    "chat-message:contact-request-test",
                    "remote-principal",
                    &body,
                    &remote.profile,
                )
                .unwrap(),
                &body,
                &remote.profile,
                self.now,
            )
            .unwrap();
        remote
            .chat
            .project_prepared_message(remote.dir.path(), &prepared, None)
            .unwrap();
        let outgoing = remote.chat.test_core().pending_outgoing(self.now).unwrap();
        let frame = remote
            .chat
            .test_core()
            .prepare_transport_frame(outgoing[0].envelope_bytes())
            .unwrap();
        self.chat
            .test_core()
            .ingest_transport_frame(&frame, self.now)
            .unwrap();
        for handoff in self.chat.pending_messages().unwrap() {
            self.chat
                .project_handoff(self.dir.path(), &handoff)
                .unwrap();
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

    fn outgoing_requests(
        &self,
    ) -> Vec<crate::collaboration_contact_store::CollaborationOutgoingContactRequest> {
        self.store.outgoing_pending_requests(self.now).unwrap()
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

fn contact_request_router(
    data_dir: &std::path::Path,
    service: &crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    chat: &crate::collaboration_product::CollaborationChatProductPort,
) -> Router {
    let mut state = test_state(data_dir);
    state.collaboration_discovery_service = Some(service.clone());
    state.collaboration_chat_product_port = Some(chat.clone());
    gateway_router(state)
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap()
}

#[tokio::test]
async fn test_chat_contact_request_refuses_callers_without_a_chat_session() {
    let home = ContactRequestHome::new().await;
    home.show(vec![home.remote.advertisement.clone()]);
    let participant_ref = home.remote.participant_ref();

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

    assert!(home.outgoing_requests().is_empty());
}

#[tokio::test]
async fn test_chat_contact_request_queues_a_request_by_did_with_discovery_off() {
    let home = ContactRequestHome::new().await;
    assert!(!home.store.discovery_enabled().unwrap());

    // Someone Chat does not list cannot be asked through a card.
    let stranger = RemotePerson::new("Stranger", home.now);
    let (status, body) = home
        .request(Some(&home.chat_token), &stranger.participant_ref())
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, NOT_LISTED);
    assert!(home.outgoing_requests().is_empty());

    // A person who posted in Community is asked by their DID and named as
    // Chat names them, from the signed Profile on their posts.
    let participant_ref = home.remote.participant_ref();
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        serde_json::json!({
            "status": "requested",
            "message": "Request sent. Remote sees it in Inbox when they next open Home.",
        })
    );
    assert!(!body.contains("did:key:"), "{body}");
    let outgoing = home.outgoing_requests();
    assert_eq!(outgoing.len(), 1);
    assert!(outgoing[0].addressed_by_did);
    assert_eq!(outgoing[0].recipient_profile_did, home.remote.did());

    // Asking again reuses the waiting request.
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        serde_json::json!({
            "status": "already_requested",
            "message": "Request sent. Waiting for Remote to accept.",
        })
    );
    assert_eq!(home.outgoing_requests().len(), 1);
    assert!(!home.store.discovery_enabled().unwrap());
}

#[tokio::test]
async fn test_chat_contact_request_binds_to_the_listing_of_a_person_visible_in_discovery() {
    let home = ContactRequestHome::new().await;
    home.show(vec![home.remote.advertisement.clone()]);
    let participant_ref = home.remote.participant_ref();

    // The v1 request an alpha.25 Home can still receive.
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        serde_json::json!({
            "status": "requested",
            "message": "Request sent. Remote sees it in Inbox when they next open Home.",
        })
    );
    let outgoing = home.outgoing_requests();
    assert_eq!(outgoing.len(), 1);
    assert!(!outgoing[0].addressed_by_did);

    // Asking again reuses the stored request.
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["status"], "already_requested");
    assert_eq!(home.outgoing_requests().len(), 1);
}

#[tokio::test]
async fn test_chat_profile_id_is_released_only_for_a_listed_person() {
    let home = ContactRequestHome::new().await;
    let participant_ref = home.remote.participant_ref();
    let body = serde_json::json!({ "participant_ref": participant_ref });

    let (status, _) = home.post(PROFILE_ID_PATH, None, body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, response) = home
        .post(
            PROFILE_ID_PATH,
            Some(&home.authority.people_token),
            body.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{response}");
    assert!(!response.contains("did:key:"), "{response}");

    let stranger = RemotePerson::new("Stranger", home.now);
    let (status, response) = home
        .post(
            PROFILE_ID_PATH,
            Some(&home.chat_token),
            serde_json::json!({ "participant_ref": stranger.participant_ref() }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(response, NOT_LISTED);

    let (status, response) = home
        .post(PROFILE_ID_PATH, Some(&home.chat_token), body)
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        json(&response),
        serde_json::json!({ "profile_id": home.remote.did() })
    );
    // Copying a DID asks nobody anything.
    assert!(home.outgoing_requests().is_empty());
}

#[tokio::test]
async fn test_people_contact_request_by_did_answers_each_status_without_echoing_the_did() {
    let home = ContactRequestHome::new().await;
    let token = home.authority.people_token.clone();
    let ask = |token: Option<String>, profile_id: String| {
        let home = &home;
        async move {
            home.post(
                PEOPLE_REQUEST_PATH,
                token.as_deref(),
                serde_json::json!({ "profile_id": profile_id }),
            )
            .await
        }
    };
    let remote_did = home.remote.did().to_string();

    // Only People's own launch token may ask.
    for other in [None, Some(home.chat_token.clone())] {
        let (status, body) = ask(other, remote_did.clone()).await;
        assert!(status.is_client_error(), "{status} {body}");
        assert!(!body.contains("did:key:"), "{body}");
    }
    assert!(home.outgoing_requests().is_empty());

    // Not a DID, or this Home's device DID: refused without an echo.
    let device_did =
        crate::collaboration_profile_authority::load_existing_device_did(home.dir.path())
            .unwrap()
            .unwrap();
    for (profile_id, expected) in [
        (
            "did:key:not-a-key".to_string(),
            "That isn't a DID. A DID starts with did:key:z.",
        ),
        (
            device_did,
            "That's this Home's device DID. Use the DID shown in People.",
        ),
    ] {
        let (status, body) = ask(Some(token.clone()), profile_id).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body, expected);
    }

    // Your own DID is answered, not sent.
    let own = home.local_profile.document().profile_did.clone();
    let (status, body) = ask(Some(token.clone()), own).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        serde_json::json!({ "status": "own_profile", "message": "That's your own DID." })
    );
    assert!(home.outgoing_requests().is_empty());

    // Anyone else's DID is queued once, whatever Discovery says, named from
    // their signed Community posts when this Home holds them.
    let (status, body) = ask(Some(token.clone()), format!("  {remote_did}  ")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        serde_json::json!({
            "status": "requested",
            "message": "Request sent. Remote sees it in Inbox when they next open Home.",
        })
    );
    assert!(!body.contains("did:key:"), "{body}");
    let (status, body) = ask(Some(token.clone()), remote_did.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["status"], "already_requested");
    assert_eq!(home.outgoing_requests().len(), 1);
    // Someone who never posted and isn't a contact has no signed name here.
    let unknown = RemotePerson::new("Unknown", home.now);
    let (status, body) = ask(Some(token.clone()), unknown.did().to_string()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        serde_json::json!({
            "status": "requested",
            "message": "Request sent. Name not known yet. They'll see it in Inbox when they next open Home.",
        })
    );
    assert_eq!(home.outgoing_requests().len(), 2);

    // People shows the waiting rows with the same names, and says when the
    // Community node cannot deliver requests by DID.
    let requested_rows = || async {
        let (status, summary) =
            home_test_get_json(&home.app, "/api/apps/people/summary", &token, "null").await;
        assert_eq!(status, StatusCode::OK, "{summary}");
        assert_eq!(
            summary["profile_id"],
            home.local_profile.document().profile_did.as_str()
        );
        assert!(!summary.to_string().contains(home.remote.did()));
        let mut rows = summary["people"]["contacts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["relationship"] == "requested")
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| row["display_name"].as_str().unwrap().to_string());
        rows
    };
    let rows = requested_rows().await;
    let names = rows
        .iter()
        .map(|row| row["display_name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            crate::collaboration_contact_store::CONTACT_NAME_NOT_KNOWN,
            "Remote"
        ]
    );
    assert!(rows.iter().all(|row| row.get("delivery_detail").is_none()));
    home.service.set_profile_relay_unsupported_for_test();
    for row in requested_rows().await {
        assert_eq!(
            row["delivery_detail"],
            crate::collaboration_discovery_runtime::PROFILE_CONTACT_UNDELIVERABLE_DETAIL
        );
    }
    // A new request is refused while that lasts.
    let stranger = RemotePerson::new("Stranger", home.now);
    let (status, body) = ask(Some(token.clone()), stranger.did().to_string()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body,
        "They may be on an older version. Ask them to update, or turn on Discovery in People on both Homes."
    );
    assert_eq!(home.outgoing_requests().len(), 2);
}

#[tokio::test]
async fn test_people_refuses_the_device_did_of_someone_who_posted_in_community() {
    // Not a contact and no presence: only their signed Community post names
    // the device their Home signs with, which System > About shows them.
    let home = ContactRequestHome::new().await;
    let device = home.remote.chat.test_core().test_local_device_did();
    assert!(home.remote.profile.authorizes_endpoint(&device));
    let (status, body) = home
        .post(
            PEOPLE_REQUEST_PATH,
            Some(&home.authority.people_token),
            serde_json::json!({ "profile_id": device }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body,
        "That's a Home's device DID. Use the DID shown in their People."
    );
    assert!(home.outgoing_requests().is_empty());
}

#[tokio::test]
async fn test_a_did_another_profile_only_declares_is_still_asked() {
    // A member lists the victim's Profile DID among their own devices and
    // posts once. The declaration proves nothing about the victim's key, so
    // asking the victim by their DID still works; the device that really
    // signed the post is refused.
    let home = ContactRequestHome::new().await;
    let victim = RemotePerson::new("Victim", home.now);
    let griefer = RemotePerson::declaring("Griefer", home.now, vec![victim.did().to_string()]);
    assert!(griefer.profile.authorizes_endpoint(victim.did()));
    home.post_in_community(&griefer);
    let ask = |profile_id: String| {
        home.post(
            PEOPLE_REQUEST_PATH,
            Some(&home.authority.people_token),
            serde_json::json!({ "profile_id": profile_id }),
        )
    };
    let (status, body) = ask(victim.did().to_string()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["status"], "requested");
    let (status, body) = ask(griefer.chat.test_core().test_local_device_did()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(home.outgoing_requests().len(), 1);
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
    home.app = contact_request_router(home.dir.path(), &home.service, &home.chat);
    home.show(vec![home.remote.advertisement.clone()]);
    let participant_ref = home.remote.participant_ref();
    let (status, body) = home.request(Some(&home.chat_token), &participant_ref).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(home.outgoing_requests().len(), 1);
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
            &home.remote.advertisement,
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
    assert_eq!(home.outgoing_requests().len(), 1);

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
    assert_eq!(home.outgoing_requests().len(), 1);
}

#[tokio::test]
async fn community_presence_choice_is_the_callers_own_and_separate_from_discovery() {
    let mut home = ContactRequestHome::new().await;
    let mut state = test_state(home.dir.path());
    state.collaboration_discovery_service = Some(home.service.clone());
    state.collaboration_presence_product_port = Some(
        crate::collaboration_presence::CollaborationPresenceProductPort::new(
            home.chat.test_core().clone(),
        )
        .unwrap()
        .with_presence_authority(crate::collaboration_presence_grant::test_authority()),
    );
    home.app = gateway_router(state);
    const CHAT_PATH: &str = "/api/apps/chat-room/community-presence/request";
    const SYSTEM_PATH: &str = "/api/apps/system/community/presence";
    let contact_file = elastos_common::localhost::rooted_localhost_fs_path(
        home.dir.path(),
        &format!(
            "{}/.AppData/ElastOS/People/contact-state.json",
            home.store.localhost_root()
        ),
    )
    .unwrap();
    let contacts_before = std::fs::read(&contact_file).ok();
    let discovery_before = home.store.discovery_enabled().unwrap();

    // No token, or a body that names anyone, changes nothing.
    let (status, _) = home
        .post(SYSTEM_PATH, None, serde_json::json!({ "mode": "always" }))
        .await;
    assert!(status.is_client_error(), "{status}");
    let (status, _) = home
        .post(
            SYSTEM_PATH,
            Some(&home.authority.system_token),
            serde_json::json!({ "mode": "always", "principal_id": "someone-else" }),
        )
        .await;
    assert!(status.is_client_error(), "{status}");
    // Chat's token cannot choose for the person.
    let (status, _) = home
        .post(
            SYSTEM_PATH,
            Some(&home.chat_token),
            serde_json::json!({ "mode": "always" }),
        )
        .await;
    assert!(status.is_client_error(), "{status}");

    // Chat only asks; the answer stays the person's.
    let (status, body) = home
        .post(CHAT_PATH, Some(&home.chat_token), serde_json::json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["community_presence"], "requested");
    assert_eq!(body["joined"], true);

    // System's row is the caller's own choice and leaves Discovery and the
    // contact store exactly as they were.
    for (mode, expected) in [
        ("always", "always"),
        ("while_open", "while_open"),
        ("hidden", "hidden"),
    ] {
        let (status, body) = home
            .post(
                SYSTEM_PATH,
                Some(&home.authority.system_token),
                serde_json::json!({ "mode": mode }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            body.contains(&format!(r#""community_presence":"{expected}""#)),
            "{body}"
        );
        assert!(!body.contains("token"), "{body}");
    }
    assert_eq!(home.store.discovery_enabled().unwrap(), discovery_before);
    assert_eq!(std::fs::read(&contact_file).ok(), contacts_before);
}
