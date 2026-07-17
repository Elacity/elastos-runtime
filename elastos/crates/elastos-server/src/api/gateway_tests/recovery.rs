use super::*;

#[tokio::test]
async fn test_legacy_recovery_kit_routes_are_absent() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    for path in [
        "/api/auth/recovery/create",
        "/api/auth/recovery/export",
        "/api/auth/recovery/import",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{path} unexpectedly resolved with {}",
            response.status()
        );
    }
}

#[tokio::test]
async fn test_full_recovery_bundle_prevents_admin_exporting_guest_root() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let guest_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &guest.proof_binding_id).unwrap();
    let intent = json!({
        "principal_id": guest.principal_id,
        "localhost_root": guest_principal.localhost_root,
        "label": "Guest root",
    });
    let fresh_token = intent_token_for_authority_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &admin,
        "auth.full-recovery-bundle.export",
        &intent,
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/full-export")
                .header("x-elastos-home-token", admin.system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.export.request/v1",
                        "principal_id": intent["principal_id"],
                        "localhost_root": intent["localhost_root"],
                        "label": intent["label"],
                        "home_token": fresh_token,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("principal binding mismatch"));
}

#[tokio::test]
async fn test_full_recovery_bundle_exports_and_restores_wallet_keys() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);

    let create_account = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", wallet_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "chain_namespace": "eip155:20",
                        "label": "Spending",
                        "create_new": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_account.status(), StatusCode::OK);
    let create_body = axum::body::to_bytes(create_account.into_body(), usize::MAX)
        .await
        .unwrap();
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).unwrap();
    let account_id = create_json["accounts"][0]["account_id"]
        .as_str()
        .unwrap()
        .to_string();

    let export_intent = json!({
        "principal_id": authority.principal_id,
        "localhost_root": principal.localhost_root,
        "label": "Everything",
    });
    let fresh_token = intent_token_for_authority_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let export = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/full-export")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.export.request/v1",
                        "principal_id": export_intent["principal_id"],
                        "localhost_root": export_intent["localhost_root"],
                        "label": export_intent["label"],
                        "home_token": fresh_token,
                        "download_password": "test password"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    let export_body = axum::body::to_bytes(export.into_body(), usize::MAX)
        .await
        .unwrap();
    let export_json: serde_json::Value = serde_json::from_slice(&export_body).unwrap();
    assert_eq!(
        export_json["schema"],
        "elastos.full-recovery-bundle.package/v1"
    );
    assert!(
        export_json["protection"]["encrypted_full_recovery_bundle"]
            .as_str()
            .unwrap_or_default()
            .len()
            > 32
    );
    assert!(!export_json.to_string().contains("private_key_hex"));

    let delete_token = intent_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.account.delete",
        &json!({ "account_id": account_id }),
    );
    let delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{account_id}"))
                .header("x-elastos-home-token", wallet_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "home_token": delete_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);

    let import = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/full-import")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.import.request/v1",
                        "principal_id": authority.principal_id,
                        "localhost_root": principal.localhost_root,
                        "package": export_json,
                        "password": "test password"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(import.status(), StatusCode::OK);
    let import_body = axum::body::to_bytes(import.into_body(), usize::MAX)
        .await
        .unwrap();
    let import_json: serde_json::Value = serde_json::from_slice(&import_body).unwrap();
    assert_eq!(
        import_json["schema"],
        "elastos.full-recovery-bundle.import.response/v1"
    );
    assert_eq!(import_json["wallet_recovery_key_count"], 1);

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", wallet_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_json: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    let restored_account = summary_json["wallet_accounts"]["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["account_id"] == account_id)
        .expect("restored wallet account");
    assert_eq!(restored_account["signing_available"], true);
    assert_eq!(restored_account["signing_status"], "managed_key_available");
}

#[tokio::test]
async fn test_full_recovery_bundle_recovers_existing_account_under_new_passkey() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let original = passkey_authority_with_name(dir.path(), Some("original"));
    let original_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &original.proof_binding_id)
            .unwrap();
    let original_wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &original);

    let create_account = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", original_wallet_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "chain_namespace": "eip155",
                        "label": "Recovered Spending",
                        "create_new": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_account.status(), StatusCode::OK);

    let export_intent = json!({
        "principal_id": original.principal_id,
        "localhost_root": original_principal.localhost_root,
        "label": "Everything",
    });
    let fresh_token = intent_token_for_authority_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &original,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let export = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/full-export")
                .header("x-elastos-home-token", original.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.export.request/v1",
                        "principal_id": export_intent["principal_id"],
                        "localhost_root": export_intent["localhost_root"],
                        "label": export_intent["label"],
                        "home_token": fresh_token,
                        "download_password": "test password"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    let export_body = axum::body::to_bytes(export.into_body(), usize::MAX)
        .await
        .unwrap();
    let export_json: serde_json::Value = serde_json::from_slice(&export_body).unwrap();

    let replacement = passkey_authority_with_name_role(
        dir.path(),
        Some("replacement"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let replacement_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &replacement.proof_binding_id)
            .unwrap();

    let import = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/full-import")
                .header("x-elastos-home-token", replacement.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.import.request/v1",
                        "principal_id": replacement.principal_id,
                        "localhost_root": replacement_principal.localhost_root,
                        "package": export_json,
                        "password": "test password",
                        "reassign_to_current_principal": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(import.status(), StatusCode::OK);
    let import_body = axum::body::to_bytes(import.into_body(), usize::MAX)
        .await
        .unwrap();
    let import_json: serde_json::Value = serde_json::from_slice(&import_body).unwrap();
    assert_eq!(import_json["status"], "reassigned");
    assert_eq!(import_json["principal_id"], original_principal.principal_id);
    assert_eq!(
        import_json["localhost_root"],
        original_principal.localhost_root
    );
    assert_eq!(
        import_json["previous_principal_id"],
        replacement_principal.principal_id
    );
    assert_eq!(import_json["wallet_recovery_key_count"], 1);
    assert!(import_json["home_token"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(
        crate::auth::load_principal_for_proof_binding(dir.path(), &original.proof_binding_id)
            .is_err()
    );
    let recovered =
        crate::auth::load_principal_for_proof_binding(dir.path(), &replacement.proof_binding_id)
            .unwrap();
    assert_eq!(recovered.principal_id, original_principal.principal_id);
    assert_eq!(recovered.localhost_root, original_principal.localhost_root);
    assert!(!crate::auth::is_auth_session_active(
        dir.path(),
        &original.session_id,
        crate::auth::now_ts()
    )
    .unwrap());
}
