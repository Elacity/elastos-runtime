use super::*;

#[tokio::test]
async fn test_recovery_kit_routes_are_principal_bound_and_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let principal_id = authority.principal_id.clone();
    let localhost_root = principal.localhost_root.clone();

    let export = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/export")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_EXPORT_REQUEST_SCHEMA,
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::FORBIDDEN);
    let export_body = axum::body::to_bytes(export.into_body(), usize::MAX)
        .await
        .unwrap();
    let export_text = String::from_utf8(export_body.to_vec()).unwrap();
    assert!(export_text.contains("principal root encryption"));
    assert!(export_text.contains("recovery protector"));

    let import = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/import")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_IMPORT_REQUEST_SCHEMA,
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                        "kit": {
                            "schema": elastos_runtime::auth::RECOVERY_KIT_SCHEMA,
                            "kit_id": "kit:route-test",
                            "protector_id": "protector:recovery:route-test",
                            "principal_id": principal_id,
                            "localhost_root": localhost_root,
                            "data_key_id": "pdek:route-test",
                            "recovery_phrase": "aaaa-bbbb-cccc-dddd-eeee-ffff-1111-2222-3333-4444",
                            "salt": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                            "nonce": "AAAAAAAAAAAAAAAA",
                            "wrapped_data_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                            "encrypted_root_descriptor": "enc:v1:metadata-ciphertext",
                            "crypto": {
                                "cipher": "aes-256-gcm",
                                "signatures": ["ed25519", "ml-dsa-65"],
                                "kems": ["x25519", "ml-kem-768"],
                                "recovery_kdf": "hkdf-sha256"
                            },
                            "created_at": 1_800_000_000u64,
                            "instructions": ["Import through ElastOS Runtime recovery."]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(import.status(), StatusCode::FORBIDDEN);
    let import_body = axum::body::to_bytes(import.into_body(), usize::MAX)
        .await
        .unwrap();
    let import_text = String::from_utf8(import_body.to_vec()).unwrap();
    assert!(
        import_text.contains("invalid recovery kit")
            || import_text.contains("unsupported encrypted root descriptor")
    );
}

#[tokio::test]
async fn test_recovery_kit_routes_reject_wallet_bound_home_session() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let now = crate::auth::now_ts();
    let wallet_binding =
        ProofBinding::evm_account(20, "0x1111111111111111111111111111111111111111", now);
    let principal = crate::auth::upsert_principal_for_binding_as_role(
        dir.path(),
        wallet_binding,
        "person:local:wallet-recovery-test".to_string(),
        crate::auth::RuntimePrincipalRole::Admin,
        now,
    )
    .unwrap();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", uuid_like_token()),
        session_id: format!("auth:{}", uuid_like_token()),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec![HOME_CAPSULE_ID.to_string(), SYSTEM_CAPSULE_ID.to_string()],
    };
    crate::auth::store_session_grant(dir.path(), grant.clone()).unwrap();
    let system_token =
        issue_home_launch_token_for_auth_grant(dir.path(), SYSTEM_CAPSULE_ID, &grant).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/recovery/status")
                .header("x-elastos-home-token", system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("passkey authority"));
}

#[tokio::test]
async fn test_recovery_kit_routes_prevent_admin_exporting_guest_kit() {
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

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/create")
                .header("x-elastos-home-token", guest.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_CREATE_REQUEST_SCHEMA,
                        "principal_id": guest.principal_id,
                        "localhost_root": guest_principal.localhost_root,
                        "label": "Guest Recovery Kit",
                        "download_password": "guest password",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);

    let export = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/export")
                .header("x-elastos-home-token", admin.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_EXPORT_REQUEST_SCHEMA,
                        "principal_id": guest.principal_id,
                        "localhost_root": guest_principal.localhost_root,
                        "download_password": "guest password",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(export.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(export.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("principal binding mismatch"));
    assert!(!text.contains("recovery_phrase"));
}

#[tokio::test]
async fn test_recovery_kit_routes_create_export_and_import_password_package() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let principal_id = authority.principal_id.clone();
    let localhost_root = principal.localhost_root.clone();
    let password = "correct horse battery";

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/create")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_CREATE_REQUEST_SCHEMA,
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                        "label": "Recovery Kit",
                        "download_password": password,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let create_body = axum::body::to_bytes(create.into_body(), usize::MAX)
        .await
        .unwrap();
    let create_json: serde_json::Value = serde_json::from_slice(&create_body).unwrap();
    assert_eq!(
        create_json["schema"],
        elastos_runtime::auth::RECOVERY_KIT_PACKAGE_SCHEMA
    );
    assert_eq!(create_json["principal_id"], principal_id);
    assert_eq!(create_json["localhost_root"], localhost_root);
    assert!(
        create_json["protection"]["encrypted_recovery_kit"]
            .as_str()
            .unwrap_or_default()
            .len()
            > 32
    );

    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/recovery/status")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = axum::body::to_bytes(status.into_body(), usize::MAX)
        .await
        .unwrap();
    let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
    assert_eq!(status_json["recovery_configured"], true);
    assert_eq!(status_json["recovery_download_available"], true);

    let export = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/export")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_EXPORT_REQUEST_SCHEMA,
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                        "download_password": password,
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
    let package: serde_json::Value = serde_json::from_slice(&export_body).unwrap();
    assert_eq!(
        package["schema"],
        elastos_runtime::auth::RECOVERY_KIT_PACKAGE_SCHEMA
    );
    assert_eq!(package["principal_id"], principal_id);
    assert_eq!(package["localhost_root"], localhost_root);

    let wrong_import = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/import")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_IMPORT_REQUEST_SCHEMA,
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                        "reassign_to_current_principal": false,
                        "package": package,
                        "password": "wrong password",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_import.status(), StatusCode::FORBIDDEN);
    let wrong_body = axum::body::to_bytes(wrong_import.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8(wrong_body.to_vec())
        .unwrap()
        .contains("invalid recovery kit package"));

    let import = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/recovery/import")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": elastos_runtime::auth::RECOVERY_KIT_IMPORT_REQUEST_SCHEMA,
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                        "reassign_to_current_principal": false,
                        "package": package,
                        "password": password,
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
    assert_eq!(import_json["status"], "imported");
    assert_eq!(import_json["principal_id"], principal_id);
    assert_eq!(import_json["localhost_root"], localhost_root);
}

#[tokio::test]
async fn test_full_recovery_bundle_exports_and_restores_wallet_keys() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
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
                        "principal_id": authority.principal_id,
                        "localhost_root": principal.localhost_root,
                        "label": "Everything",
                        "home_token": authority.home_token,
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

    let delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{account_id}"))
                .header("x-elastos-home-token", wallet_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "home_token": authority.home_token }).to_string(),
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
                        "principal_id": original.principal_id,
                        "localhost_root": original_principal.localhost_root,
                        "label": "Everything",
                        "home_token": original.home_token,
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
