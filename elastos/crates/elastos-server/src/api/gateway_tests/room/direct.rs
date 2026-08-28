use super::*;

struct DirectApiRouteFixture {
    dir: tempfile::TempDir,
    app: Router,
    chat_token: String,
    chat_context: HomeLaunchTokenContext,
    wrong_capsule_token: String,
    session_id: String,
    profile: crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    peer: crate::collaboration_discovery_runtime::tests::DirectGatewayPeerFixture,
}

async fn direct_api_route_fixture() -> DirectApiRouteFixture {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Local Person"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let (identity_key, _) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let local_key = elastos_runtime::signature::SigningKey::from_bytes(&identity_key.to_bytes());
    let profile = crate::collaboration_profile_authority::update_profile_authority(
        dir.path(),
        &authority.principal_id,
        &protection.localhost_root,
        &authority.proof_binding_id,
        "Local Person",
        None,
        crate::auth::now_ts(),
    )
    .unwrap();
    let peer = crate::collaboration_discovery_runtime::tests::direct_gateway_peer_fixture(
        dir.path(),
        &authority.principal_id,
        &protection.localhost_root,
        &local_key,
        &profile,
    )
    .await;
    peer.service
        .register_sync_context(
            peer.store.clone(),
            profile.clone(),
            &authority.session_id,
            Some(&authority.proof_binding_id),
            &authority.grant_id,
            crate::auth::now_ts(),
        )
        .unwrap();
    let mut state = test_state(dir.path());
    state.collaboration_discovery_service = Some(peer.service.clone());
    let app = gateway_router(state);
    // Mint the chat window token the way the shell does: a projection token
    // under Home authority against the person's real session grant, which
    // enumerates authority actors ("home"), never the executable capsule.
    let chat_token = issue_home_projection_launch_token_with_context(
        dir.path(),
        CHAT_ROOM_CAPSULE_ID,
        CHAT_ROOM_CAPSULE_ID,
        &HomeLaunchTokenContext {
            principal_id: authority.principal_id.clone(),
            session_id: authority.session_id.clone(),
            proof_binding_id: Some(authority.proof_binding_id.clone()),
            grant_id: authority.grant_id.clone(),
        },
    )
    .unwrap();
    let mut chat_headers = HeaderMap::new();
    chat_headers.insert(HOST, HeaderValue::from_static("localhost:61180"));
    chat_headers.insert("origin", HeaderValue::from_static("null"));
    chat_headers.insert(
        "x-elastos-home-token",
        HeaderValue::from_str(&chat_token).unwrap(),
    );
    let chat_context =
        require_home_launch_token_context(dir.path(), &chat_headers, CHAT_ROOM_CAPSULE_ID).unwrap();
    let session_id = chat_context.session_id.clone();
    DirectApiRouteFixture {
        profile: profile.clone(),
        dir,
        app,
        chat_token,
        chat_context,
        wrong_capsule_token: authority.people_token,
        session_id,
        peer,
    }
}

fn direct_api_request(token: Option<&str>, method: &str, uri: &str, body: Body) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(HOST, "localhost:61180")
        .header("origin", "null");
    if let Some(token) = token {
        request = request.header("x-elastos-home-token", token);
    }
    request
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn json_response(response: Response) -> serde_json::Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn wait_for_direct_peer_ready(
    peer: &crate::collaboration_discovery_runtime::tests::DirectGatewayPeerFixture,
    remote_node: &crate::carrier::CarrierNode,
) {
    // Probe over the same plane the delivery path uses: the fixture's local
    // node endpoint plus an explicitly seeded remote address. Dialing a bare
    // endpoint id from a fresh anonymous endpoint would depend on live
    // mDNS/DNS discovery, which is a different resolution plane than the
    // code under test.
    use iroh::Watcher as _;
    let remote_addr = remote_node.endpoint.watch_addr().get();
    peer._local_node
        .memory_lookup
        .add_endpoint_info(remote_addr.clone());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let provider_ready = peer.remote_registry.schemes().await.iter().any(|scheme| {
                scheme == crate::collaboration_direct_messages::DIRECT_MESSAGE_PROVIDER_SCHEME
            });
            if provider_ready
                && crate::carrier::CarrierClient::connect_known_endpoint(
                    &peer._local_node.endpoint,
                    remote_addr.clone(),
                    1,
                )
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("direct Carrier peer/provider readiness deadline exceeded");
}

async fn direct_send(
    fixture: &DirectApiRouteFixture,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "POST",
            "/api/apps/chat-room/direct/messages/send",
            Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    let status = response.status();
    (status, json_response(response).await)
}

#[tokio::test]
async fn direct_api_auth_list_and_message_projection_are_bounded_and_redacted() {
    let fixture = direct_api_route_fixture().await;
    for token in [None, Some(fixture.wrong_capsule_token.as_str())] {
        let response = fixture
            .app
            .clone()
            .oneshot(direct_api_request(
                token,
                "GET",
                "/api/apps/chat-room/direct/conversations",
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let expired_token = issue_expired_home_launch_token_with_context(
        fixture.dir.path(),
        CHAT_ROOM_CAPSULE_ID,
        &fixture.chat_context,
    )
    .unwrap();
    let expired = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&expired_token),
            "GET",
            "/api/apps/chat-room/direct/conversations",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);

    let list = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            "/api/apps/chat-room/direct/conversations",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list = json_response(list).await;
    assert_eq!(list["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(
        list["conversations"][0],
        json!({
            "conversation_id": fixture.peer.conversation_id,
            "display_name": "Remote Person",
            "removed": false,
        })
    );
    let list_text = list.to_string();
    for forbidden in [
        "did:key",
        "device",
        "principal",
        "session",
        "provider",
        "route",
    ] {
        assert!(!list_text.contains(forbidden));
    }

    let direct = fixture.peer.service.direct_message_service();
    let local_profile_did = fixture.peer.store.local_profile_did().to_string();
    let remote_did = fixture.peer.store.snapshot().unwrap().contacts()[0]
        .remote_presence_device_did()
        .to_string();
    let now = crate::auth::now_ts();
    for index in 0..201 {
        direct
            .persist_outgoing_for_test(
                &local_profile_did,
                &format!("read-model-{index:03}"),
                &fixture.peer.conversation_id,
                &remote_did,
                &format!("message {index}"),
                now - 300 + index,
            )
            .unwrap();
    }
    let group_before = room_store_snapshot(fixture.dir.path());
    let messages = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            &format!(
                "/api/apps/chat-room/direct/conversations/{}/messages",
                fixture.peer.conversation_id
            ),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(messages.status(), StatusCode::OK);
    let messages = json_response(messages).await;
    assert_eq!(messages["messages"].as_array().unwrap().len(), 200);
    let message = &messages["messages"][0];
    assert_eq!(
        message
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            "created_at",
            "delivery_state",
            "direction",
            "message_id",
            "text",
        ]
    );
    let message_text = messages.to_string();
    for forbidden in [
        "did:key",
        "signature",
        "receipt",
        "envelope",
        "carrier",
        "provider",
        "route",
    ] {
        assert!(!message_text.contains(forbidden));
    }
    assert_eq!(room_store_snapshot(fixture.dir.path()), group_before);

    crate::auth::revoke_session_grant(
        &home_launch_auth_data_dir(fixture.dir.path()),
        &fixture.session_id,
        crate::auth::now_ts(),
    )
    .unwrap();
    let revoked = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            "/api/apps/chat-room/direct/conversations",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    // The chat window shares the person's real session, so revoking it also
    // retires the registered delivery context — background direct-message
    // authority fails closed with the session instead of outliving it.
    assert!(fixture
        .peer
        .service
        .direct_message_service()
        .records_for_test(
            fixture.peer.store.local_profile_did(),
            crate::auth::now_ts(),
        )
        .is_err());
}

#[tokio::test]
async fn direct_api_send_is_strict_idempotent_and_contact_gated() {
    let fixture = direct_api_route_fixture().await;
    wait_for_direct_peer_ready(&fixture.peer, &fixture.peer._remote_node).await;
    let direct = fixture.peer.service.direct_message_service();
    let local_profile_did = fixture.peer.store.local_profile_did().to_string();
    let conversation_id = fixture.peer.conversation_id.clone();
    let group_before = room_store_snapshot(fixture.dir.path());
    let before = direct
        .records_for_test(&local_profile_did, crate::auth::now_ts())
        .unwrap();

    let malformed = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "POST",
            "/api/apps/chat-room/direct/messages/send",
            Body::from("{"),
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    for invalid in [
        json!({"request_id":"","conversation_id":conversation_id,"text":"hello"}),
        json!({"request_id":"strict-request","conversation_id":conversation_id,"text":""}),
        json!({
            "request_id":"strict-request",
            "conversation_id":conversation_id,
            "text":"hello",
            "device_did":"did:key:caller",
            "profile_did":"did:key:caller",
            "network_id":"caller-network",
            "provider":"caller-provider",
            "route":"caller-route"
        }),
        json!({
            "request_id":"strict-request",
            "conversation_id":conversation_id,
            "text":"x".repeat(8 * 1024 + 1)
        }),
    ] {
        assert_eq!(
            direct_send(&fixture, invalid).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let substituted = json!({
        "request_id":"strict-request",
        "conversation_id":"direct:v1:substituted",
        "text":"hello"
    });
    assert_eq!(
        direct_send(&fixture, substituted).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        direct
            .records_for_test(&local_profile_did, crate::auth::now_ts())
            .unwrap(),
        before
    );

    let exact = json!({
        "request_id":"strict-request",
        "conversation_id":conversation_id,
        "text":"hello"
    });
    let (status, body) = direct_send(&fixture, exact.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"status":"receipt_settled"}));
    let first_records = direct
        .records_for_test(&local_profile_did, crate::auth::now_ts())
        .unwrap();
    assert_eq!(
        first_records
            .iter()
            .filter(|record| !record.incoming)
            .count(),
        1
    );
    assert!(first_records.iter().all(|record| record.receipt_settled));
    assert_eq!(direct_send(&fixture, exact).await.0, StatusCode::OK);
    assert_eq!(
        direct
            .records_for_test(&local_profile_did, crate::auth::now_ts())
            .unwrap(),
        first_records
    );
    assert_eq!(
        direct_send(
            &fixture,
            json!({
                "request_id":"strict-request",
                "conversation_id":conversation_id,
                "text":"changed"
            }),
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    let contact = fixture.peer.store.snapshot().unwrap().contacts()[0].clone();
    fixture
        .peer
        .service
        .remove_contact(
            &fixture.peer.store,
            &fixture.profile,
            contact.remote_profile_did(),
            crate::auth::now_ts(),
        )
        .await
        .unwrap();
    // The declared read policy keeps history readable after removal — a
    // removed relationship owns its conversation instead of orphaning it.
    let readable = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            &format!("/api/apps/chat-room/direct/conversations/{conversation_id}/messages"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(readable.status(), StatusCode::OK);
    let listed = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            "/api/apps/chat-room/direct/conversations",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = json_response(listed).await;
    let listed_conversation = listed["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|conversation| conversation["conversation_id"] == conversation_id)
        .expect("removed conversation stays listed");
    assert_eq!(listed_conversation["removed"], true);
    assert_eq!(
        direct_send(
            &fixture,
            json!({
                "request_id":"after-remove",
                "conversation_id":conversation_id,
                "text":"blocked"
            }),
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        direct
            .records_for_test(&local_profile_did, crate::auth::now_ts())
            .unwrap(),
        first_records
    );
    assert_eq!(room_store_snapshot(fixture.dir.path()), group_before);
}

#[tokio::test]
async fn direct_api_pending_retry_settles_the_same_durable_envelope() {
    let fixture = direct_api_route_fixture().await;
    wait_for_direct_peer_ready(&fixture.peer, &fixture.peer._remote_node).await;
    fixture.peer._remote_node.endpoint.close().await;
    let direct = fixture.peer.service.direct_message_service();
    let local_profile_did = fixture.peer.store.local_profile_did().to_string();
    let body = json!({
        "request_id":"pending-request",
        "conversation_id":fixture.peer.conversation_id,
        "text":"retry me"
    });
    let (status, response) = direct_send(&fixture, body.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{response}");
    assert_eq!(response, json!({"status":"pending"}));
    let pending = direct
        .records_for_test(&local_profile_did, crate::auth::now_ts())
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].receipt_settled);
    let exact_envelope = pending[0].envelope_bytes.clone();

    let remote_did = crate::crypto::encode_signing_key_did(&fixture.peer.remote_key);
    let restarted = crate::carrier::start_carrier_node_with_registry(
        &fixture.peer.remote_key,
        &remote_did,
        fixture.dir.path().join("direct-api-remote-restarted"),
        Some(Arc::downgrade(&fixture.peer.remote_registry)),
    )
    .await
    .unwrap();
    wait_for_direct_peer_ready(&fixture.peer, &restarted).await;
    direct
        .retry_pending(&local_profile_did, crate::auth::now_ts())
        .await
        .unwrap();
    let settled = direct
        .records_for_test(&local_profile_did, crate::auth::now_ts())
        .unwrap();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].envelope_bytes, exact_envelope);
    assert!(settled[0].receipt_settled);
    assert_eq!(direct_send(&fixture, body).await.0, StatusCode::OK);
    assert_eq!(
        direct
            .records_for_test(&local_profile_did, crate::auth::now_ts())
            .unwrap(),
        settled
    );
}

#[tokio::test]
async fn direct_api_authority_configuration_and_corruption_fail_closed() {
    let fixture = direct_api_route_fixture().await;

    let missing_service = gateway_router(test_state(fixture.dir.path()))
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            "/api/apps/chat-room/direct/conversations",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(missing_service.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let empty = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(empty.path(), Some("Setup Name"));
    let token = app_token_for_authority(empty.path(), CHAT_ROOM_CAPSULE_ID, &authority);
    let mut state = test_state(empty.path());
    state.collaboration_discovery_service = Some(fixture.peer.service.clone());
    let identity_path = empty.path().join("identity/device.key");
    let identity_before = std::fs::read(&identity_path).unwrap();
    let root =
        crate::auth::store_test_principal_root_protection(empty.path(), &authority.principal_id);
    let profile_uri =
        crate::collaboration_profile_authority::profile_authority_object_uri(&root.localhost_root);
    let profile_path =
        elastos_common::localhost::rooted_localhost_fs_path(empty.path(), &profile_uri).unwrap();
    let contact_path = elastos_common::localhost::rooted_localhost_fs_path(
        empty.path(),
        &format!(
            "{}/.AppData/ElastOS/People/contact-state.json",
            root.localhost_root
        ),
    )
    .unwrap();
    let direct_path = elastos_common::localhost::rooted_localhost_fs_path(
        empty.path(),
        &format!(
            "{}/.AppData/ElastOS/Chat/direct-messages.json",
            root.localhost_root
        ),
    )
    .unwrap();
    let absent_profile = gateway_router(state)
        .oneshot(direct_api_request(
            Some(&token),
            "GET",
            "/api/apps/chat-room/direct/conversations",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(absent_profile.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(std::fs::read(identity_path).unwrap(), identity_before);
    assert!(!profile_path.exists());
    assert!(!contact_path.exists());
    assert!(!direct_path.exists());

    let direct = fixture.peer.service.direct_message_service();
    direct
        .persist_outgoing_for_test(
            fixture.peer.store.local_profile_did(),
            "corrupt-read-model",
            &fixture.peer.conversation_id,
            fixture.peer.store.snapshot().unwrap().contacts()[0].remote_presence_device_did(),
            "stored",
            crate::auth::now_ts(),
        )
        .unwrap();
    let uri = format!(
        "{}/.AppData/ElastOS/Chat/direct-messages.json",
        fixture.peer.store.localhost_root()
    );
    let path =
        elastos_common::localhost::rooted_localhost_fs_path(fixture.dir.path(), &uri).unwrap();
    crate::auth::write_protected_principal_root_object(
        fixture.dir.path(),
        fixture.peer.store.principal_id(),
        fixture.peer.store.localhost_root(),
        &uri,
        &path,
        b"{}",
    )
    .unwrap();
    let corrupt_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        direct_send(
            &fixture,
            json!({
                "request_id":"corrupt-send",
                "conversation_id":fixture.peer.conversation_id,
                "text":"must not replace corrupt state"
            }),
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(std::fs::read(&path).unwrap(), corrupt_bytes);
    let corrupted = fixture
        .app
        .clone()
        .oneshot(direct_api_request(
            Some(&fixture.chat_token),
            "GET",
            &format!(
                "/api/apps/chat-room/direct/conversations/{}/messages",
                fixture.peer.conversation_id
            ),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(corrupted.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let error = json_response(corrupted).await.to_string();
    assert!(!error.contains(fixture.dir.path().to_string_lossy().as_ref()));
    assert!(!error.contains("did:key"));
}
