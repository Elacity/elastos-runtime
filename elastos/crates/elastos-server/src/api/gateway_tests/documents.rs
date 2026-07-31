use super::*;

#[tokio::test]
async fn test_documents_provider_routes_require_home_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);

    let denied = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_inbox_routes_require_home_token_and_return_notifications() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let token = issue_home_launch_token(dir.path(), INBOX_CAPSULE_ID).unwrap();
    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["app"]["id"], INBOX_CAPSULE_ID);
    assert_eq!(summary_payload["app"]["route"], "/apps/inbox/");
    assert_eq!(summary_payload["notifications"]["unread_count"], 0);

    let bad_action = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"action_id":"unknown-action"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_action.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_documents_provider_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), DOCUMENTS_CAPSULE_ID).unwrap();

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Provider Notes"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let created_payload: serde_json::Value = serde_json::from_slice(&created_body).unwrap();
    assert_eq!(created_payload["status"], "ok");
    let doc_did = created_payload["data"]["document"]["doc_did"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        created_payload["data"]["document"]["document_uri"],
        format!("localhost://ElastOS/Documents/{}", doc_did)
    );

    let saved = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/save")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": doc_did,
                        "title": "Provider Notes",
                        "body": "# Provider Notes\n\nSaved through provider.\n",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["status"], "ok");
    assert!(summary_payload["data"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| {
            item["doc_did"] == doc_did
                && item["title"] == "Provider Notes"
                && item["document_uri"] == format!("localhost://ElastOS/Documents/{}", doc_did)
        }));

    let fetched = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/get")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": doc_did,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    let fetched_body = axum::body::to_bytes(fetched.into_body(), usize::MAX)
        .await
        .unwrap();
    let fetched_payload: serde_json::Value = serde_json::from_slice(&fetched_body).unwrap();
    assert_eq!(fetched_payload["status"], "ok");
    assert_eq!(
        fetched_payload["data"]["document"]["body"],
        "# Provider Notes\n\nSaved through provider.\n"
    );
    assert_eq!(
        fetched_payload["data"]["document"]["document_uri"],
        format!("localhost://ElastOS/Documents/{}", doc_did)
    );

    let deleted = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/delete")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": doc_did,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted_body = axum::body::to_bytes(deleted.into_body(), usize::MAX)
        .await
        .unwrap();
    let deleted_payload: serde_json::Value = serde_json::from_slice(&deleted_body).unwrap();
    assert_eq!(deleted_payload["status"], "ok");

    let summary_after_delete = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary_after_delete.status(), StatusCode::OK);
    let summary_after_delete_body =
        axum::body::to_bytes(summary_after_delete.into_body(), usize::MAX)
            .await
            .unwrap();
    let summary_after_delete_payload: serde_json::Value =
        serde_json::from_slice(&summary_after_delete_body).unwrap();
    assert!(!summary_after_delete_payload["data"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["doc_did"] == doc_did));
}

#[tokio::test]
async fn test_documents_provider_scopes_documents_to_launch_principal() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &admin);
    let guest_token = app_token_for_authority(dir.path(), DOCUMENTS_CAPSULE_ID, &guest);
    let admin_root = crate::auth::principal_localhost_root(&admin.principal_id);
    let guest_root = crate::auth::principal_localhost_root(&guest.principal_id);

    let admin_created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", admin_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Shared Title"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_created.status(), StatusCode::OK);
    let admin_body = axum::body::to_bytes(admin_created.into_body(), usize::MAX)
        .await
        .unwrap();
    let admin_payload: serde_json::Value = serde_json::from_slice(&admin_body).unwrap();
    let admin_doc_did = admin_payload["data"]["document"]["doc_did"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        admin_payload["data"]["document"]["working_copy_uri"],
        format!("{admin_root}/Documents/shared-title.md")
    );

    let guest_created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", guest_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Shared Title"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_created.status(), StatusCode::OK);
    let guest_body = axum::body::to_bytes(guest_created.into_body(), usize::MAX)
        .await
        .unwrap();
    let guest_payload: serde_json::Value = serde_json::from_slice(&guest_body).unwrap();
    let guest_doc_did = guest_payload["data"]["document"]["doc_did"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(admin_doc_did, guest_doc_did);
    assert_eq!(
        guest_payload["data"]["document"]["working_copy_uri"],
        format!("{guest_root}/Documents/shared-title.md")
    );

    let admin_summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", admin_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_summary.status(), StatusCode::OK);
    let admin_summary_body = axum::body::to_bytes(admin_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let admin_summary_payload: serde_json::Value =
        serde_json::from_slice(&admin_summary_body).unwrap();
    let admin_docs = admin_summary_payload["data"]["documents"]
        .as_array()
        .unwrap();
    assert_eq!(admin_docs.len(), 1);
    assert_eq!(admin_docs[0]["doc_did"], admin_doc_did);

    let guest_fetch_admin = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/get")
                .header("x-elastos-home-token", guest_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "doc_did": admin_doc_did,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_fetch_admin.status(), StatusCode::OK);
    let denied_body = axum::body::to_bytes(guest_fetch_admin.into_body(), usize::MAX)
        .await
        .unwrap();
    let denied_payload: serde_json::Value = serde_json::from_slice(&denied_body).unwrap();
    assert_eq!(denied_payload["status"], "error");
    assert_eq!(
        denied_payload["message"],
        "document does not belong to this principal"
    );
}

#[tokio::test]
async fn test_library_home_token_can_read_documents_provider_summary() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/summary")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_payload["status"], "ok");
    assert!(summary_payload["data"]["documents"].is_array());
}

#[tokio::test]
async fn test_library_home_token_cannot_mutate_documents_provider() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(documents_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), LIBRARY_CAPSULE_ID).unwrap();

    let denied = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/documents/create")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Should Fail"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_body = axum::body::to_bytes(denied.into_body(), usize::MAX)
        .await
        .unwrap();
    let denied_text = String::from_utf8(denied_body.to_vec()).unwrap();
    assert!(denied_text.contains("home launch token is not authorized for this provider"));
}

#[tokio::test]
async fn test_viewer_gateway_routes_list_and_serve_viewer_bound_capsules() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), "gba-emulator");
    write_test_browser_capsule(dir.path(), "other-viewer", "viewer", "Other viewer", None);
    write_test_viewer_capsule(
        dir.path(),
        "demo-rom",
        "gba-emulator",
        "demo.gba",
        "Demo ROM",
    );
    write_test_viewer_capsule(
        dir.path(),
        "demo-rom-2",
        "gba-emulator",
        "demo-2.gba",
        "Demo ROM 2",
    );
    write_test_viewer_capsule(
        dir.path(),
        "other-rom",
        "other-viewer",
        "other.gba",
        "Other ROM",
    );
    let app = gateway_router(test_state(dir.path()));
    let home_token = issue_home_launch_token(dir.path(), GBA_EMULATOR_CAPSULE_ID).unwrap();

    let denied_library = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/gba-emulator/library")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied_library.status(), StatusCode::UNAUTHORIZED);

    let library = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/gba-emulator/library")
                .header("x-elastos-home-token", home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(library.status(), StatusCode::OK);
    let body = axum::body::to_bytes(library.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let capsules = payload["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["capsule"].as_str())
        .collect::<Vec<_>>();
    assert!(capsules.contains(&"demo-rom"));
    assert!(capsules.contains(&"demo-rom-2"));
    assert!(capsules.contains(&"gba-ucity"));
    assert_eq!(payload["items"][0]["entrypoint"], "demo.gba");

    let rom_home_token = issue_home_projection_launch_token_with_context(
        dir.path(),
        "demo-rom",
        GBA_EMULATOR_CAPSULE_ID,
        &local_home_launch_token_context(dir.path()).unwrap(),
    )
    .unwrap();
    let rom_library = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/gba-emulator/library")
                .header("x-elastos-home-token", rom_home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rom_library.status(), StatusCode::OK);
    let rom_library_body = axum::body::to_bytes(rom_library.into_body(), usize::MAX)
        .await
        .unwrap();
    let rom_library_payload: serde_json::Value = serde_json::from_slice(&rom_library_body).unwrap();
    let rom_capsules = rom_library_payload["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["capsule"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(rom_capsules, vec!["demo-rom"]);

    let content = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/gba-emulator/content/demo-rom")
                .header("x-elastos-home-token", rom_home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
    assert_eq!(
        content
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/octet-stream")
    );
    let body = axum::body::to_bytes(content.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"rom-data");

    let wrong_viewer_token = issue_home_projection_launch_token_with_context(
        dir.path(),
        "other-rom",
        "other-viewer",
        &local_home_launch_token_context(dir.path()).unwrap(),
    )
    .unwrap();
    let wrong_viewer = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/gba-emulator/content/other-rom")
                .header("x-elastos-home-token", wrong_viewer_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_viewer.status(), StatusCode::UNAUTHORIZED);

    let non_viewer = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/documents/library")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(non_viewer.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_viewer_gateway_storage_routes_require_home_token_and_round_trip_bytes() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), "gba-emulator");
    write_test_viewer_capsule(
        dir.path(),
        "demo-rom",
        "gba-emulator",
        "demo.gba",
        "Demo ROM",
    );
    let home_token = issue_home_launch_token(dir.path(), "gba-emulator").unwrap();
    let app = gateway_router(test_state(dir.path()));

    let unauthorized = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri("/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1")
                .body(Body::from("save-state"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let put = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri("/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1")
                .header("x-elastos-home-token", home_token.clone())
                .body(Body::from("save-state"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

    let get = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1")
                .header("x-elastos-home-token", home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    let body = axum::body::to_bytes(get.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"save-state");
}

#[tokio::test]
async fn test_viewer_gateway_storage_scopes_users_self_to_launch_principal() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), "gba-emulator");
    write_test_viewer_capsule(
        dir.path(),
        "demo-rom",
        "gba-emulator",
        "demo.gba",
        "Demo ROM",
    );
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &admin.principal_id);
    let admin_token = app_token_for_authority(dir.path(), "gba-emulator", &admin);
    let guest_token = app_token_for_authority(dir.path(), "gba-emulator", &guest);
    let app = gateway_router(test_state(dir.path()));
    let storage_uri = "/api/viewers/gba-emulator/storage/demo-rom/state/demo.ss1";

    let admin_put = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(storage_uri)
                .header("x-elastos-home-token", admin_token.clone())
                .body(Body::from("admin-state"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_put.status(), StatusCode::NO_CONTENT);

    let guest_put = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(storage_uri)
                .header("x-elastos-home-token", guest_token.clone())
                .body(Body::from("guest-state"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_put.status(), StatusCode::NO_CONTENT);

    let admin_get = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(storage_uri)
                .header("x-elastos-home-token", admin_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_get.status(), StatusCode::OK);
    let admin_body = axum::body::to_bytes(admin_get.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&admin_body[..], b"admin-state");

    let guest_get = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri(storage_uri)
                .header("x-elastos-home-token", guest_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_get.status(), StatusCode::OK);
    let guest_body = axum::body::to_bytes(guest_get.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&guest_body[..], b"guest-state");

    let admin_root = crate::auth::principal_localhost_root(&admin.principal_id);
    let guest_root = crate::auth::principal_localhost_root(&guest.principal_id);
    let admin_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!("{admin_root}/.AppData/LocalHost/GBA/test/states/demo.ss1"),
    )
    .unwrap();
    assert!(admin_path.is_file());
    let admin_stored = std::fs::read_to_string(&admin_path).unwrap();
    assert!(!admin_stored.contains("admin-state"));
    assert!(admin_stored.contains("elastos.principal-root.object/v1"));
    assert!(admin_stored.contains(&admin_protection.localhost_root));
    assert!(elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!("{guest_root}/.AppData/LocalHost/GBA/test/states/demo.ss1")
    )
    .unwrap()
    .is_file());
    assert!(!dir
        .path()
        .join("Users/self/.AppData/LocalHost/GBA/test/states/demo.ss1")
        .exists());
}
