use super::*;

fn configured_presence_state(
    data_dir: &std::path::Path,
) -> (
    GatewayState,
    crate::collaboration_presence::CollaborationPresenceProductPort,
) {
    let chat = crate::collaboration_product::test_chat_product_port(
        data_dir,
        "gateway-presence-network",
        "gateway-presence-conversation",
    );
    let presence = crate::collaboration_presence::CollaborationPresenceProductPort::new(
        chat.test_core().clone(),
    )
    .unwrap()
    .with_presence_authority(crate::collaboration_presence_grant::test_authority());
    let mut state = test_state(data_dir);
    state.collaboration_chat_product_port = Some(chat);
    state.collaboration_presence_product_port = Some(presence.clone());
    (state, presence)
}

async fn response_json(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn response_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn file_snapshot(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &std::path::Path, path: &std::path::Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, out);
            } else if entry.file_type().unwrap().is_file() {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn capsule_request(method: &str, uri: &str, token: &str, body: impl Into<Body>) -> Request<Body> {
    let origin = if uri.starts_with("/api/apps/home/") {
        "http://localhost:61180"
    } else {
        "null"
    };
    test_browser_request("localhost:61180", origin)
        .method(method)
        .uri(uri)
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(body.into())
        .unwrap()
}

fn project_remote_presence(
    local: &crate::collaboration_presence::CollaborationPresenceProductPort,
    data_root: &std::path::Path,
    display_name: &str,
    now: u64,
) -> String {
    let remote_chat = crate::collaboration_product::test_chat_product_port(
        data_root,
        "gateway-presence-network",
        "gateway-presence-conversation",
    );
    let remote = crate::collaboration_presence::CollaborationPresenceProductPort::new(
        remote_chat.test_core().clone(),
    )
    .unwrap();
    let remote_profile = remote_chat.test_person_profile(display_name, None);
    let prepared = remote
        .prepare_presence(
            crate::collaboration_presence::presence_request_binding(
                "remote-presence",
                "remote-principal",
                &remote_profile,
            )
            .unwrap(),
            &remote_profile,
            now,
        )
        .unwrap();
    let sender_profile_did = prepared.sender_profile_did().to_string();
    local
        .test_core()
        .accept_incoming_from_signed_source_for_test(prepared.test_envelope_bytes(), now)
        .unwrap();
    let handoff = local
        .pending_presences()
        .unwrap()
        .into_iter()
        .find(|handoff| handoff.sender_profile_did() == sender_profile_did)
        .unwrap();
    local.project_handoff(&handoff, now).unwrap();
    sender_profile_did
}

fn install_signed_profile(
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
    display_name: &str,
) {
    crate::auth::store_test_principal_root_protection(data_dir, &authority.principal_id);
    crate::collaboration_profile_authority::update_profile_authority(
        data_dir,
        &authority.principal_id,
        &crate::auth::principal_localhost_root(&authority.principal_id),
        &authority.proof_binding_id,
        display_name,
        None,
        crate::auth::now_ts(),
    )
    .unwrap();
}

#[test]
fn room_attribution_name_map_never_upgrades_false_or_none_rows_after_reopen() {
    let local_dir = tempfile::tempdir().unwrap();
    let (_, local_presence) = configured_presence_state(local_dir.path());
    let now = crate::auth::now_ts();
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_did = project_remote_presence(&local_presence, remote_dir.path(), "Remote", now);
    let snapshot = local_presence.snapshot(now).unwrap();
    assert_eq!(snapshot.records().len(), 1);
    assert_eq!(snapshot.records()[0].display_name(), "Remote");

    let named_did = "did:key:zNamedByProfileHead".to_string();
    let wrong_profile_did = "did:key:zWrongProfileHead".to_string();
    let names = std::collections::HashMap::from([
        (named_did.clone(), "Signed Name".to_string()),
        (remote_did.clone(), "Remote Signed".to_string()),
        (wrong_profile_did.clone(), "Wrong Profile".to_string()),
    ]);
    let participant =
        |display_name: &str, member_did: Option<String>, profile_verified: Option<bool>| {
            crate::room_service::ParticipantView {
                profile_verified,
                display_name: display_name.to_string(),
                device_label: "ElastOS device".to_string(),
                last_seen_at: now,
                member_did,
                role: None,
                local_session_count: 0,
                is_current_session: false,
            }
        };
    let object = |seq: u64,
                  sender: &str,
                  member_did: Option<String>,
                  sender_profile_verified: Option<bool>| {
        crate::room_service::ConversationObjectView {
            delivery_state: None,
            sender_profile_verified,
            seq,
            sender: sender.to_string(),
            sender_member_did: member_did,
            from_current_session: false,
            kind: crate::room_service::ConversationObjectKind::Text,
            body: Some("historical".to_string()),
            emoji: None,
            link: None,
            attachment: None,
            created_at: now - 1,
        }
    };
    let poll = || crate::room_service::RoomPollView {
        own_delivery: Vec::new(),
        room_slug: "chat-room".to_string(),
        display_name: "Local".to_string(),
        expires_at: now + 300,
        latest_seq: 4,
        participants: vec![
            participant("Device fallback", Some(named_did.clone()), Some(true)),
            participant("Device fallback", Some(remote_did.clone()), Some(false)),
            participant(
                "Wrong profile session",
                Some(wrong_profile_did.clone()),
                Some(false),
            ),
            participant("Invited guest", None, None),
        ],
        objects: vec![
            object(1, "Device 1234abcd", Some(named_did.clone()), Some(true)),
            object(2, "Device 5678ef01", Some(remote_did.clone()), Some(false)),
            object(
                3,
                "Wrong profile device",
                Some(wrong_profile_did.clone()),
                Some(false),
            ),
            object(4, "Invited guest", None, None),
        ],
        transport: crate::room_service::RoomTransportView::default(),
    };
    let before_reads = file_snapshot(local_dir.path());
    for _ in 0..2 {
        let mut reopened = poll();
        gateway_room::apply_profile_attribution_to_room_poll(&mut reopened, &names);
        // A signed Profile head names the sender.
        assert_eq!(reopened.objects[0].sender, "Signed Name");
        assert_eq!(reopened.objects[0].sender_profile_verified, Some(true));
        assert_eq!(reopened.participants[0].display_name, "Signed Name");
        assert_eq!(reopened.participants[0].profile_verified, Some(true));
        // False/None rows stay false/None here. This test proves only that the
        // gateway name map does not upgrade authority; room-service authority
        // semantics are covered by its own tests.
        assert_eq!(reopened.objects[1].sender, "Device 5678ef01");
        assert_eq!(reopened.objects[1].sender_profile_verified, Some(false));
        assert_eq!(reopened.participants[1].display_name, "Device fallback");
        assert_eq!(reopened.participants[1].profile_verified, Some(false));
        assert_eq!(reopened.objects[2].sender, "Wrong profile device");
        assert_eq!(reopened.objects[2].sender_profile_verified, Some(false));
        assert_eq!(
            reopened.participants[2].display_name,
            "Wrong profile session"
        );
        assert_eq!(reopened.participants[2].profile_verified, Some(false));
        // Session rows without a member DID keep their guest/session identity;
        // configured product code filters them later instead of upgrading them.
        assert_eq!(reopened.objects[3].sender, "Invited guest");
        assert_eq!(reopened.objects[3].sender_profile_verified, None);
        assert_eq!(reopened.participants[3].display_name, "Invited guest");
        assert_eq!(reopened.participants[3].profile_verified, None);
    }
    assert_eq!(file_snapshot(local_dir.path()), before_reads);
}

#[tokio::test]
async fn configured_room_poll_renders_signed_profile_names_only() {
    const NETWORK: &str = "gateway-presence-network";
    let dir = tempfile::tempdir().unwrap();
    let (mut state, _presence) = configured_presence_state(dir.path());
    let chat_port = state.collaboration_chat_product_port.clone().unwrap();
    let (runtime_device_key, _) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let (trusted_key, _) = elastos_runtime::signature::generate_keypair();
    state.collaboration_discovery_service = Some(
        crate::collaboration_discovery_runtime::CollaborationDiscoveryService::new(
            ed25519_dalek::SigningKey::from_bytes(&runtime_device_key.to_bytes()),
            super::home_system::configured_discovery_network_profile_for_test(
                &trusted_key,
                NETWORK,
            ),
            Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
        )
        .await
        .unwrap(),
    );
    let authority = passkey_authority_with_name(dir.path(), Some("Owner"));
    install_signed_profile(dir.path(), &authority, "Owner Signed");
    let now = crate::auth::now_ts();
    let app = gateway_router(state);

    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let payload = response_json(launch).await;
    let chat_token = test_launch_token_from_route(payload["route"].as_str().unwrap());

    let send = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/objects/send",
            &chat_token,
            r#"{"request_id":"chat-message:attribution-local","body":"local hello"}"#,
        ))
        .await
        .unwrap();
    let send_status = send.status();
    let send_body = response_text(send).await;
    assert_eq!(send_status, StatusCode::OK, "{send_body}");

    // A second runtime in the same conversation sends a signed text. Its
    // device signature verifies, but no accepted Profile head, membership
    // card, or local identity names it.
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_port = crate::collaboration_product::test_chat_product_port(
        remote_dir.path(),
        "gateway-presence-network",
        "gateway-presence-conversation",
    );
    let remote_profile = remote_port.test_person_profile("Remote Signed", None);
    let remote_prepared = remote_port
        .prepare_message(
            crate::collaboration_product::chat_message_request_binding(
                "chat-message:attribution-remote",
                "remote-principal",
                "remote hello",
                &remote_profile,
            )
            .unwrap(),
            "remote hello",
            &remote_profile,
            now,
        )
        .unwrap();
    remote_port
        .project_prepared_message(remote_dir.path(), &remote_prepared, None)
        .unwrap();
    let outgoing = remote_port.test_core().pending_outgoing(now).unwrap();
    assert_eq!(outgoing.len(), 1);
    let transport_frame = remote_port
        .test_core()
        .prepare_transport_frame(outgoing[0].envelope_bytes())
        .unwrap();
    let ingestion = chat_port
        .test_core()
        .ingest_transport_frame(&transport_frame, now)
        .unwrap();
    assert!(matches!(
        ingestion,
        crate::collaboration_core::CollaborationTransportIngestion::Incoming(_)
    ));
    let handoffs = chat_port.pending_messages().unwrap();
    assert_eq!(handoffs.len(), 1);
    chat_port.project_handoff(dir.path(), &handoffs[0]).unwrap();

    let poll = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/poll",
            &chat_token,
            r#"{"since":0}"#,
        ))
        .await
        .unwrap();
    assert_eq!(poll.status(), StatusCode::OK);
    let payload = response_json(poll).await;
    let objects = payload["objects"].as_array().cloned().unwrap_or_default();
    let local_object = objects
        .iter()
        .find(|object| object["body"].as_str() == Some("local hello"))
        .expect("local text projected");
    assert_eq!(local_object["sender"].as_str(), Some("Owner Signed"));
    assert_eq!(local_object["sender_profile_verified"], json!(true));
    let remote_object = objects
        .iter()
        .find(|object| object["body"].as_str() == Some("remote hello"))
        .expect("remote text projected");
    assert_eq!(remote_object["sender"].as_str(), Some("Remote Signed"));
    assert_eq!(remote_object["sender_profile_verified"], json!(true));

    let participants = payload["participants"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(participants
        .iter()
        .all(|participant| participant.get("member_did").is_none()));
    let remote_participant = participants
        .iter()
        .find(|participant| participant["display_name"].as_str() == Some("Remote Signed"))
        .expect("remote participant listed");
    assert_eq!(
        remote_participant["display_name"].as_str(),
        Some("Remote Signed")
    );
    assert_eq!(remote_participant["profile_verified"], json!(true));
    let local_participant = participants
        .iter()
        .find(|participant| participant["display_name"].as_str() == Some("Owner Signed"))
        .expect("local participant listed");
    assert_eq!(
        local_participant["display_name"].as_str(),
        Some("Owner Signed")
    );
    assert_eq!(local_participant["profile_verified"], json!(true));
}

#[tokio::test]
async fn home_summary_ignores_legacy_people_contacts_without_profile_store() {
    let local_dir = tempfile::tempdir().unwrap();
    let (state, local_presence) = configured_presence_state(local_dir.path());
    let authority = passkey_authority_with_name(local_dir.path(), Some("Local"));
    let now = crate::auth::now_ts();
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_did = project_remote_presence(&local_presence, remote_dir.path(), "Remote", now);
    home_system::write_home_principal_object_json_for_authority(
        local_dir.path(),
        &authority,
        "people-contacts.json",
        json!({
            "schema": "elastos.people.contacts-state/v1",
            "principal_id": authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&authority.principal_id),
            "updated_at": now,
            "contacts": {
                "contact:remote": {
                    "contact_id": "contact:remote",
                    "peer_id": "legacy-route",
                    "did": remote_did,
                    "display_name": "ElastOS user",
                    "added_at": now,
                    "updated_at": now,
                    "source": "legacy"
                }
            }
        }),
    );
    let app = gateway_router(state);
    let summary_request = || {
        test_browser_request("localhost:61180", "http://localhost:61180")
            .uri("/api/apps/home/summary")
            .header("x-elastos-home-token", authority.home_token.clone())
            .body(Body::empty())
            .unwrap()
    };
    // Summary reads ignore the legacy Services peer object without migrating
    // it or projecting it as People authority.
    let before_read = file_snapshot(local_dir.path());
    let response = app.clone().oneshot(summary_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["people"]["contact_count"], 0);
    assert!(payload["people"]["contacts"].as_array().unwrap().is_empty());
    assert!(payload["people"]["service_offers"]
        .as_array()
        .unwrap()
        .is_empty());
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let migrated = elastos_common::localhost::rooted_localhost_fs_path(
        local_dir.path(),
        &format!("{localhost_root}/.AppData/ElastOS/Home/services-peer-contacts.json"),
    )
    .unwrap();
    let legacy = elastos_common::localhost::rooted_localhost_fs_path(
        local_dir.path(),
        &format!("{localhost_root}/.AppData/ElastOS/Home/people-contacts.json"),
    )
    .unwrap();
    assert!(!migrated.exists());
    assert!(legacy.is_file());
    assert_eq!(file_snapshot(local_dir.path()), before_read);

    // Explicit authenticated launch owns the one-time Services object rename.
    let launch = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/launch")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"inbox"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    assert!(migrated.is_file());
    assert!(!legacy.exists());

    // Migration is one-time, and every later summary remains byte-pure.
    let steady_state = file_snapshot(local_dir.path());
    let response = app.clone().oneshot(summary_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload = response_json(response).await;
    assert_eq!(payload["people"]["contact_count"], 0);
    assert_eq!(file_snapshot(local_dir.path()), steady_state);
}

#[tokio::test]
async fn collaboration_presence_routes_are_capsule_scoped_and_unconfigured_is_pure() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Alice"));
    let chat_token = app_token_for_authority(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let unbound_chat_token = issue_home_launch_token(dir.path(), CHAT_ROOM_CAPSULE_ID).unwrap();
    let before = file_snapshot(dir.path());
    let app = gateway_router(state);

    assert_eq!(
        app.clone()
            .oneshot(capsule_request(
                "POST",
                "/api/apps/chat-room/presence",
                &unbound_chat_token,
                "{}",
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    let heartbeat = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/presence",
            &chat_token,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_text(heartbeat).await,
        "Save your Profile before turning on Discovery"
    );
    assert_eq!(file_snapshot(dir.path()), before);

    install_signed_profile(dir.path(), &authority, "Alice");
    let after_profile = file_snapshot(dir.path());
    let heartbeat = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/presence",
            &chat_token,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), StatusCode::OK);
    assert_eq!(
        response_json(heartbeat).await,
        json!({
            "configured": false,
            "queued": false,
            "next_heartbeat_after_ms": 15_000,
        })
    );
    let snapshot = app
        .clone()
        .oneshot(capsule_request(
            "GET",
            "/api/apps/people/presence",
            &authority.people_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot = response_json(snapshot).await;
    assert_eq!(snapshot["configured"], false);
    assert_eq!(snapshot["online_count"], 0);
    assert_eq!(snapshot["online"], json!([]));
    assert_eq!(file_snapshot(dir.path()), after_profile);
    assert!(!dir.path().join("collaboration").exists());

    // Only Chat's own launch token drives the heartbeat; the Home shell's
    // old route is gone.
    for token in [
        &authority.home_token,
        &authority.people_token,
        &authority.system_token,
    ] {
        assert_eq!(
            app.clone()
                .oneshot(capsule_request(
                    "POST",
                    "/api/apps/chat-room/presence",
                    token,
                    "{}",
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_ne!(
        app.clone()
            .oneshot(capsule_request(
                "POST",
                "/api/apps/home/collaboration/presence",
                &authority.home_token,
                "{}",
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for token in [&authority.home_token, &authority.system_token] {
        assert_eq!(
            app.clone()
                .oneshot(capsule_request(
                    "GET",
                    "/api/apps/people/presence",
                    token,
                    Body::empty(),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

async fn set_system_presence(app: &Router, token: &str, mode: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/system/community/presence",
            token,
            json!({ "mode": mode }).to_string(),
        ))
        .await
        .unwrap();
    let status = response.status();
    (status, response_text(response).await)
}

#[tokio::test]
async fn heartbeat_is_strict_proof_bound_and_coalesces_exact_presentation() {
    let dir = tempfile::tempdir().unwrap();
    let (state, presence) = configured_presence_state(dir.path());
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Alice"));
    install_signed_profile(dir.path(), &authority, "Alice");
    let chat_token = app_token_for_authority(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let app = gateway_router(state);
    let heartbeat = || {
        app.clone().oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/presence",
            &chat_token,
            "{}",
        ))
    };

    for body in [
        r#"{"request_id":"caller"}"#,
        r#"{"device_did":"did:key:caller"}"#,
        r#"{"display_name":"Caller"}"#,
        r#"{"principal":"caller"}"#,
        r#"{"network_id":"caller"}"#,
        r#"{"conversation_id":"caller"}"#,
        r#"{"created_at":1}"#,
        r#"{ }"#,
    ] {
        assert_eq!(
            app.clone()
                .oneshot(capsule_request(
                    "POST",
                    "/api/apps/chat-room/presence",
                    &chat_token,
                    body,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
            "body {body} was accepted"
        );
    }
    assert_eq!(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/chat-room/presence")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    // Having a Profile is not consent to be shown. Until the person grants
    // it, an authorized heartbeat succeeds and announces nothing.
    let quiet = heartbeat().await.unwrap();
    assert_eq!(quiet.status(), StatusCode::OK);
    assert_eq!(response_json(quiet).await["queued"], false);
    assert!(presence
        .pending_outgoing_presences(crate::auth::now_ts())
        .unwrap()
        .is_empty());

    // "Always" is the Runtime timer's grant: Chat asks only for
    // `/while-open` and does not present it.
    let (status, body) = set_system_presence(&app, &authority.system_token, "always").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        response_json(heartbeat().await.unwrap()).await["queued"],
        false
    );

    let (status, body) = set_system_presence(&app, &authority.system_token, "while_open").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The status route reports the mode, never the token.
    let status_body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status_body["community_presence"], "while_open");
    assert!(!body.contains("token"), "{body}");
    let response = heartbeat().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["queued"], true);
    let now = crate::auth::now_ts();
    assert_eq!(presence.pending_outgoing_presences(now).unwrap().len(), 1);

    crate::auth::set_principal_display_name(
        dir.path(),
        &authority.proof_binding_id,
        "Alice Updated",
        now + 1,
    )
    .unwrap();
    assert_eq!(heartbeat().await.unwrap().status(), StatusCode::OK);
    assert_eq!(presence.pending_outgoing_presences(now).unwrap().len(), 2);

    // Hidden revokes: the very next heartbeat announces nothing.
    let (status, body) = set_system_presence(&app, &authority.system_token, "hidden").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        response_json(heartbeat().await.unwrap()).await["queued"],
        false
    );

    // An epoch bump silences the grant on the next heartbeat too.
    let (status, _) = set_system_presence(&app, &authority.system_token, "while_open").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response_json(heartbeat().await.unwrap()).await["queued"],
        true
    );
    presence
        .presence_authority()
        .unwrap()
        .manager()
        .revoke_all("test epoch");
    assert_eq!(
        response_json(heartbeat().await.unwrap()).await["queued"],
        false
    );
}

#[tokio::test]
async fn only_the_persons_inbox_or_system_answer_mints_a_presence_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (state, presence) = configured_presence_state(dir.path());
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Alice"));
    install_signed_profile(dir.path(), &authority, "Alice");
    let chat_token = app_token_for_authority(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let app = gateway_router(state);
    let grants = || {
        presence
            .presence_authority()
            .unwrap()
            .manager()
            .audit_log()
            .recent_events(10_000)
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    elastos_runtime::primitives::audit::AuditEvent::CapabilityGrant { .. }
                )
            })
            .count()
    };

    // Chat cannot choose: the System route refuses its token.
    let (status, _) = set_system_presence(&app, &chat_token, "always").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(grants(), 0);

    // Chat files the request; it mints nothing.
    let asked = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/community-presence/request",
            &chat_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(asked.status(), StatusCode::OK);
    assert_eq!(
        response_json(asked).await["community_presence"],
        "requested"
    );
    assert_eq!(grants(), 0);

    let summary = app
        .clone()
        .oneshot(capsule_request(
            "GET",
            "/api/apps/inbox/summary",
            &inbox_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    let summary = response_json(summary).await;
    let card = summary["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "community_presence_request")
        .expect("the Inbox shows the request")
        .clone();
    assert!(!card.to_string().contains("token\":"), "{card}");
    let always = card["action_ref"]["action_id"].as_str().unwrap();
    let issued_at = always.strip_prefix("community-presence-always:").unwrap();
    let inbox_action = |token: &str, action: String| {
        app.clone().oneshot(capsule_request(
            "POST",
            "/api/apps/inbox/actions",
            token,
            json!({ "action_id": action }).to_string(),
        ))
    };

    // The Inbox action route takes only Inbox's token.
    let wrong = inbox_action(&chat_token, format!("community-presence-open:{issued_at}"))
        .await
        .unwrap();
    assert_ne!(wrong.status(), StatusCode::OK);
    assert_eq!(grants(), 0);

    let answered = inbox_action(&inbox_token, format!("community-presence-open:{issued_at}"))
        .await
        .unwrap();
    assert_eq!(answered.status(), StatusCode::OK);
    assert_eq!(grants(), 1);
    // The answered card is closed.
    let again = inbox_action(
        &inbox_token,
        format!("community-presence-always:{issued_at}"),
    )
    .await
    .unwrap();
    assert_ne!(again.status(), StatusCode::OK);
    assert_eq!(grants(), 1);

    let status = app
        .clone()
        .oneshot(capsule_request(
            "GET",
            "/api/apps/system/community/presence",
            &authority.system_token,
            Body::empty(),
        ))
        .await
        .unwrap();
    let body = response_text(status).await;
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["community_presence"], "while_open");
    assert!(!body.contains("token"), "{body}");
}

#[tokio::test]
async fn corrupt_profile_state_is_an_internal_presence_error_without_detail() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Alice"));
    install_signed_profile(dir.path(), &authority, "Alice");
    let authorization_request = capsule_request(
        "POST",
        "/api/apps/home/collaboration/presence",
        &authority.home_token,
        "{}",
    );
    let context = require_home_token_context(dir.path(), authorization_request.headers()).unwrap();
    let profile_path =
        crate::api::gateway::gateway_home_system::home_profile_authority_path(dir.path(), &context)
            .unwrap();
    std::fs::create_dir_all(profile_path.parent().unwrap()).unwrap();
    std::fs::write(profile_path, b"{corrupt").unwrap();

    let chat_token = app_token_for_authority(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let response = gateway_router(state)
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/presence",
            &chat_token,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let text = response_text(response).await;
    assert_eq!(text, "Home presence presentation is unavailable");
    assert!(!text.contains("signature"));
    assert!(!text.contains("seed"));
    assert!(!text.contains("did:key:"));
    assert!(!text.contains("sha256:"));
}

#[tokio::test]
async fn people_snapshot_filters_local_and_exposes_only_signed_live_remote_presence() {
    let local_dir = tempfile::tempdir().unwrap();
    let (state, local_presence) = configured_presence_state(local_dir.path());
    let authority = passkey_authority_with_profile(local_dir.path(), "Local");
    let local_profile = crate::collaboration_profile_authority::load_profile_authority(
        local_dir.path(),
        &authority.principal_id,
        &crate::auth::principal_localhost_root(&authority.principal_id),
    )
    .unwrap()
    .unwrap();
    let now = crate::auth::now_ts();
    let local_binding = crate::collaboration_presence::presence_request_binding(
        "local-route-presence",
        &authority.principal_id,
        &local_profile,
    )
    .unwrap();
    let local = local_presence
        .prepare_presence(local_binding, &local_profile, now)
        .unwrap();
    local_presence
        .project_prepared_presence(&local, now)
        .unwrap();

    let remote_dir = tempfile::tempdir().unwrap();
    let remote_chat = crate::collaboration_product::test_chat_product_port(
        remote_dir.path(),
        "gateway-presence-network",
        "gateway-presence-conversation",
    );
    let remote_presence = crate::collaboration_presence::CollaborationPresenceProductPort::new(
        remote_chat.test_core().clone(),
    )
    .unwrap();
    let remote_profile = remote_chat.test_person_profile("Remote", Some("remote"));
    let remote_binding = crate::collaboration_presence::presence_request_binding(
        "remote-route-presence",
        "remote-principal",
        &remote_profile,
    )
    .unwrap();
    let remote = remote_presence
        .prepare_presence(remote_binding, &remote_profile, now)
        .unwrap();
    local_presence
        .test_core()
        .accept_incoming_from_signed_source_for_test(remote.test_envelope_bytes(), now)
        .unwrap();
    let handoff = local_presence.pending_presences().unwrap().remove(0);
    local_presence.project_handoff(&handoff, now).unwrap();

    let app = gateway_router(state);
    use crate::collaboration_presence_grant::{set_presence_grant, PresenceDecision};
    for (decision, epoch, own_visible) in [
        (None, false, false),
        (Some(PresenceDecision::Always), false, true),
        (None, true, false),
        (Some(PresenceDecision::Hidden), false, false),
    ] {
        if let Some(decision) = decision {
            set_presence_grant(
                local_presence.presence_authority().unwrap(),
                local_dir.path(),
                &authority.principal_id,
                &local_profile.document().profile_did,
                &local_presence.presence_scope(),
                decision,
                now,
            )
            .await
            .unwrap();
        }
        if matches!(decision, Some(PresenceDecision::Always)) {
            let grant = crate::collaboration_presence_grant::presence_grant(
                local_presence.presence_authority().unwrap(),
                local_dir.path(),
                &authority.principal_id,
                &local_profile.document().profile_did,
                &local_presence.presence_scope(),
                crate::collaboration_presence_grant::PresencePublisher::Runtime,
                now,
            )
            .await
            .unwrap();
            let prepared = local_presence
                .prepare_presence(
                    crate::collaboration_presence::presence_request_binding(
                        &grant.bind_request_id("local-route-presence"),
                        &authority.principal_id,
                        &local_profile,
                    )
                    .unwrap(),
                    &local_profile,
                    now + 1,
                )
                .unwrap();
            local_presence
                .project_authorized_presence(&prepared, now + 1)
                .await
                .unwrap();
        }
        if epoch {
            local_presence
                .presence_authority()
                .unwrap()
                .manager()
                .revoke_all("read-only snapshot epoch");
        }
        let state_before = file_snapshot(local_dir.path());
        let snapshot = local_presence.authorized_snapshot(now).await.unwrap();
        assert_eq!(
            snapshot.records().iter().any(|record| {
                record.sender_profile_did() == local_profile.document().profile_did
            }),
            own_visible
        );
        let response = app
            .clone()
            .oneshot(capsule_request(
                "GET",
                "/api/apps/people/presence",
                &authority.people_token,
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value = response_json(response).await;
        assert_eq!(value["schema"], "elastos.people.presence.snapshot/v1");
        assert_eq!(value["configured"], true);
        assert_eq!(value["online_count"], 1);
        assert_eq!(value["online"][0]["display_name"], "Remote");
        assert_eq!(value["online"][0]["handle"], "remote");
        let keys = value["online"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "profile_did",
                "display_name",
                "handle",
                "last_seen_at",
                "expires_at",
            ])
        );
        assert_ne!(
            value["online"][0]["profile_did"],
            local_profile.document().profile_did
        );
        assert_eq!(file_snapshot(local_dir.path()), state_before);
    }
}

#[tokio::test]
async fn system_mode_changes_retire_projected_and_queued_presence_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (state, presence) = configured_presence_state(dir.path());
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Alice"));
    install_signed_profile(dir.path(), &authority, "Alice");
    let chat_token = app_token_for_authority(dir.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let app = gateway_router(state);
    let now = crate::auth::now_ts();
    assert_eq!(
        set_system_presence(&app, &authority.system_token, "always")
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        crate::api::gateway::publish_runtime_owned_presence(dir.path(), &presence, now)
            .await
            .unwrap(),
        1
    );
    let pending = presence.pending_outgoing_presences(now).unwrap();
    presence
        .project_authorized_presence(&pending[0], now)
        .await
        .unwrap();
    assert_eq!(
        presence
            .authorized_snapshot(now)
            .await
            .unwrap()
            .records()
            .len(),
        1
    );

    // A mode switch retires the previous answer before its new publisher runs.
    assert_eq!(
        set_system_presence(&app, &authority.system_token, "while_open")
            .await
            .0,
        StatusCode::OK
    );
    assert!(presence.snapshot(now).unwrap().records().is_empty());
    assert!(presence
        .test_core()
        .pending_outgoing(now)
        .unwrap()
        .is_empty());
    let heartbeat = app
        .clone()
        .oneshot(capsule_request(
            "POST",
            "/api/apps/chat-room/presence",
            &chat_token,
            "{}",
        ))
        .await
        .unwrap();
    assert_eq!(response_json(heartbeat).await["queued"], true);
    let pending = presence.pending_outgoing_presences(now).unwrap();
    presence
        .project_authorized_presence(&pending[0], now)
        .await
        .unwrap();
    assert_eq!(presence.snapshot(now).unwrap().records().len(), 1);

    assert_eq!(
        set_system_presence(&app, &authority.system_token, "hidden")
            .await
            .0,
        StatusCode::OK
    );
    assert!(presence.snapshot(now).unwrap().records().is_empty());
    assert!(presence
        .test_core()
        .pending_outgoing(now)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn restarted_presence_get_keeps_legacy_core_bytes_until_lifecycle_repair() {
    use crate::collaboration_presence_grant::{
        presence_grant, set_presence_grant, PresenceDecision, PresencePublisher,
    };
    let dir = tempfile::tempdir().unwrap();
    let (_, presence) = configured_presence_state(dir.path());
    let authority = passkey_authority_with_profile(dir.path(), "Local");
    let profile = crate::collaboration_profile_authority::load_profile_authority(
        dir.path(),
        &authority.principal_id,
        &crate::auth::principal_localhost_root(&authority.principal_id),
    )
    .unwrap()
    .unwrap();
    let now = crate::auth::now_ts();
    set_presence_grant(
        presence.presence_authority().unwrap(),
        dir.path(),
        &authority.principal_id,
        &profile.document().profile_did,
        &presence.presence_scope(),
        PresenceDecision::Always,
        now,
    )
    .await
    .unwrap();
    let grant = presence_grant(
        presence.presence_authority().unwrap(),
        dir.path(),
        &authority.principal_id,
        &profile.document().profile_did,
        &presence.presence_scope(),
        PresencePublisher::Runtime,
        now,
    )
    .await
    .unwrap();
    let prepared = presence
        .prepare_presence(
            crate::collaboration_presence::presence_request_binding(
                &grant.bind_request_id("restart-local-presence"),
                &authority.principal_id,
                &profile,
            )
            .unwrap(),
            &profile,
            now,
        )
        .unwrap();
    presence
        .project_authorized_presence(&prepared, now)
        .await
        .unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    let (_, remote) = configured_presence_state(remote_dir.path());
    presence.test_core().test_store_legacy_product_handoffs(
        remote.test_core(),
        &[
            "elastos.chat.message/v1",
            crate::collaboration_presence::PRESENCE_PAYLOAD_TYPE,
        ],
        now,
    );
    // Reopen Core and the presence port before any lifecycle reader. Their
    // in-memory principal map and verified-state cache both start empty.
    let (mut state, reopened) = configured_presence_state(dir.path());
    let reopened = reopened.with_presence_authority(presence.presence_authority().unwrap().clone());
    reopened
        .presence_authority()
        .unwrap()
        .manager()
        .revoke_all("restart GET denial");
    state.collaboration_presence_product_port = Some(reopened.clone());
    let before = file_snapshot(dir.path());
    assert!(reopened
        .authorized_snapshot(now)
        .await
        .unwrap()
        .records()
        .is_empty());
    let app = gateway_router(state);
    for uri in [
        "/api/apps/people/presence",
        "/api/apps/system/community/presence",
    ] {
        let token = if uri.contains("/people/") {
            &authority.people_token
        } else {
            &authority.system_token
        };
        let response = app
            .clone()
            .oneshot(capsule_request("GET", uri, token, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value = response_json(response).await;
        if uri.contains("/people/") {
            assert_eq!(value["online_count"], 0);
            assert_eq!(value["online"], serde_json::json!([]));
        }
        assert_eq!(file_snapshot(dir.path()), before);
    }
    assert!(reopened
        .test_core()
        .pending_product_handoffs()
        .unwrap()
        .is_empty());
    assert_eq!(reopened.test_core().summary().unwrap().replay_tombstones, 2);
    assert_ne!(
        file_snapshot(dir.path()),
        before,
        "lifecycle owns durable legacy repair"
    );
    assert!(reopened
        .authorized_snapshot(now)
        .await
        .unwrap()
        .records()
        .is_empty());
}
