use super::*;

fn decode_recorded_wallet_requests(requests: &[serde_json::Value]) -> Vec<WalletProviderRequestV2> {
    requests
        .iter()
        .filter(|request| request.get("op").and_then(Value::as_str) == Some(WALLET_BUS_OPERATION))
        .map(|request| {
            WalletProviderRequestV2::decode_at(
                &serde_json::to_vec(
                    request
                        .get("request")
                        .expect("recorded Wallet Bus request envelope"),
                )
                .unwrap(),
                crate::auth::now_ts(),
            )
            .unwrap()
        })
        .collect()
}

fn assert_wallet_authority(request: &WalletProviderRequestV2, expected: &RuntimeWalletAuthority) {
    let expected = expected.verified_context();
    assert_eq!(request.authority.principal_id, expected.principal_id());
    assert_eq!(request.authority.session_id, expected.session_id());
    assert_eq!(
        request.authority.proof_binding_id.as_deref(),
        expected.proof_binding_id()
    );
    assert_eq!(request.authority.grant_id, expected.grant_id());
    assert_eq!(request.authority.actor, expected.actor());
    assert_eq!(request.authority.launch_id, expected.launch_id());
}

async fn export_full_recovery_bundle_response(
    app: &axum::Router,
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
    principal: &crate::auth::PrincipalRecord,
) -> Response {
    let intent = json!({
        "principal_id": authority.principal_id,
        "localhost_root": principal.localhost_root,
        "label": "Recovery test",
        "download_password": null,
    });
    let step_up = step_up_token_for_app_context(
        data_dir,
        SYSTEM_CAPSULE_ID,
        &authority.system_token,
        "auth.full-recovery-bundle.export",
        &intent,
    );
    let mut request = intent.clone();
    request["schema"] = json!("elastos.full-recovery-bundle.export.request/v1");
    request["step_up_token"] = json!(step_up);
    app.clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/auth/recovery/full-export")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn create_and_export_raw_full_recovery_bundle(
    app: &axum::Router,
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
    principal: &crate::auth::PrincipalRecord,
) -> Value {
    let wallet_token = app_token_for_authority(data_dir, WALLET_CAPSULE_ID, authority);
    let create = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", wallet_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "chain_namespace": "eip155:20",
                        "label": "Recovery test",
                        "create_new": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);

    let export = export_full_recovery_bundle_response(app, data_dir, authority, principal).await;
    assert_eq!(export.status(), StatusCode::OK);
    let body = axum::body::to_bytes(export.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn full_recovery_export_marks_handoff_only_after_the_complete_bundle_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();

    let unavailable = gateway_router(
        wallet_test_state_with_shared_provider(
            dir.path(),
            Arc::new(RejectingFullRecoveryWalletProvider),
        )
        .await,
    );
    let failed =
        export_full_recovery_bundle_response(&unavailable, dir.path(), &authority, &principal)
            .await;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let retained = crate::auth::load_principal_root_protection(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .expect("System retains the exact staged Recovery Kit after later export failure");
    let retained_protector = retained.protectors.first().unwrap();
    assert!(retained_protector.verified_at.is_none());
    let retained_protector_id = retained_protector.protector_id.clone();
    let retained_archive = retained_protector.archive.clone();

    let ready = gateway_router(wallet_test_state(dir.path()).await);
    let exported =
        export_full_recovery_bundle_response(&ready, dir.path(), &authority, &principal).await;
    assert_eq!(exported.status(), StatusCode::OK);

    let handed = crate::auth::load_principal_root_protection(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .expect("successful System export keeps root protection");
    let handed_protector = handed.protectors.first().unwrap();
    assert_eq!(handed_protector.protector_id, retained_protector_id);
    assert_eq!(handed_protector.archive, retained_archive);
    assert!(handed_protector.verified_at.is_some());

    let restarted = gateway_router(test_state(dir.path()));
    let status = restarted
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/auth/recovery/status")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let body = axum::body::to_bytes(status.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["recovery_configured"], true);
    assert!(payload["required_actions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn full_recovery_setup_preserves_identity_and_requires_successful_profile_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let ready = gateway_router(wallet_test_state(dir.path()).await);
    // An older, verified kit remains useful for the root, but contains no Profile.
    let old =
        export_full_recovery_bundle_response(&ready, dir.path(), &authority, &principal).await;
    assert_eq!(old.status(), StatusCode::OK);
    let old: Value = serde_json::from_slice(
        &axum::body::to_bytes(old.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(old["included"]["people_identity"], false);
    let (status, _) = super::home_system::home_test_post_json(
        &ready,
        "/api/apps/people/profile",
        &authority.people_token,
        "null",
        json!({"display_name": "Edited Name"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let unavailable = gateway_router(
        wallet_test_state_with_shared_provider(
            dir.path(),
            Arc::new(RejectingFullRecoveryWalletProvider),
        )
        .await,
    );
    let failed =
        export_full_recovery_bundle_response(&unavailable, dir.path(), &authority, &principal)
            .await;
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let profile = crate::collaboration_profile_authority::load_profile_authority(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .expect("Profile exists before Wallet export");
    assert_eq!(profile.document().display_name, "Edited Name");
    let status = crate::api::auth_gateway::principal_root_recovery_status_for_verified_principal(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap();
    assert!(crate::api::auth_gateway::principal_root_recovery_is_ready(
        &status
    ));
    assert!(status
        .required_actions
        .iter()
        .any(|action| action == "download_recovery_kit_with_profile"));
    let restarted = gateway_router(wallet_test_state(dir.path()).await);
    let (first, second) = tokio::join!(
        export_full_recovery_bundle_response(&restarted, dir.path(), &authority, &principal,),
        export_full_recovery_bundle_response(&restarted, dir.path(), &authority, &principal,),
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let after = crate::collaboration_profile_authority::load_profile_authority(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        profile, after,
        "retry never renames or revises an existing Profile"
    );
    let status = crate::api::auth_gateway::principal_root_recovery_status_for_verified_principal(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap();
    assert!(status.required_actions.is_empty());
}

#[test]
fn full_recovery_setup_concurrent_initial_profile_keeps_one_key() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    crate::auth::store_test_principal_root_protection(dir.path(), &principal.principal_id);
    let before = crate::api::gateway::gateway_home_runtime::home_state(dir.path());
    let barrier = std::sync::Barrier::new(2);
    let profiles = std::thread::scope(|scope| {
        let create = || {
            barrier.wait();
            crate::collaboration_profile_authority::ensure_initial_profile_authority(
                dir.path(),
                &principal.principal_id,
                &principal.localhost_root,
                &authority.proof_binding_id,
                "Alex",
                crate::auth::now_ts(),
            )
            .unwrap()
        };
        let first = scope.spawn(create);
        let second = scope.spawn(create);
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!(profiles.0, profiles.1);
    assert_eq!(profiles.0.document().revision, 1);
    let after = crate::api::gateway::gateway_home_runtime::home_state(dir.path());
    assert_eq!(
        serde_json::to_value(before.people).unwrap(),
        serde_json::to_value(after.people).unwrap()
    );
}

#[tokio::test]
async fn full_recovery_export_rejects_changed_intent_and_wrong_actor_before_effects() {
    for wrong_actor in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let authority = passkey_authority_with_name(dir.path(), Some("alex"));
        let principal =
            crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
                .unwrap();
        let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
        let app = gateway_router(wallet_test_state(dir.path()).await);
        let mut intent = json!({"principal_id": principal.principal_id, "localhost_root": principal.localhost_root,
            "label": "Recovery test", "download_password": null});
        let step_up = step_up_token_for_app_context(
            dir.path(),
            SYSTEM_CAPSULE_ID,
            &authority.system_token,
            "auth.full-recovery-bundle.export",
            &intent,
        );
        if !wrong_actor {
            intent["label"] = json!("Changed");
        }
        intent["schema"] = json!("elastos.full-recovery-bundle.export.request/v1");
        intent["step_up_token"] = json!(step_up);
        let token = if wrong_actor {
            app_token_for_authority(dir.path(), PEOPLE_CAPSULE_ID, &authority)
        } else {
            authority.system_token.clone()
        };
        let response = app
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/auth/recovery/full-export")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(intent.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.status().is_success());
        assert!(crate::auth::load_principal_root_protection(
            dir.path(),
            &principal.principal_id,
            &principal.localhost_root
        )
        .unwrap()
        .is_none());
        assert!(
            crate::collaboration_profile_authority::load_profile_authority(
                dir.path(),
                &principal.principal_id,
                &principal.localhost_root
            )
            .unwrap()
            .is_none()
        );
    }
}

struct RejectingFullRecoveryWalletProvider;

#[async_trait::async_trait]
impl Provider for RejectingFullRecoveryWalletProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "rejecting recovery test provider supports only raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["wallet"]
    }

    fn name(&self) -> &'static str {
        "rejecting-full-recovery-wallet-provider"
    }

    async fn send_raw(
        &self,
        _request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        Err(ProviderError::Provider(
            "simulated Wallet recovery export failure".into(),
        ))
    }
}

async fn import_raw_full_recovery_bundle(
    app: &axum::Router,
    token: &str,
    principal_id: &str,
    localhost_root: &str,
    bundle: &Value,
    reassign: bool,
) -> (StatusCode, Value) {
    import_raw_full_recovery_bundle_with_terminal_retry(
        app,
        token,
        principal_id,
        localhost_root,
        bundle,
        reassign,
        None,
    )
    .await
}

async fn import_raw_full_recovery_bundle_with_terminal_retry(
    app: &axum::Router,
    token: &str,
    principal_id: &str,
    localhost_root: &str,
    bundle: &Value,
    reassign: bool,
    terminal_retry_token: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = test_browser_request("localhost:61180", "null")
        .method("POST")
        .uri("/api/auth/recovery/full-import")
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json");
    if let Some(terminal_retry_token) = terminal_retry_token {
        request = request.header("x-elastos-recovery-terminal", terminal_retry_token);
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.import.request/v1",
                        "principal_id": principal_id,
                        "localhost_root": localhost_root,
                        "bundle": bundle,
                        "reassign_to_current_principal": reassign,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| {
            json!({
                "error": String::from_utf8_lossy(&body),
            })
        }),
    )
}

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
        "download_password": null,
    });
    let fresh_token = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &admin.system_token,
        "auth.full-recovery-bundle.export",
        &intent,
    );

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
                        "step_up_token": fresh_token,
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
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);

    let create_account = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
        "download_password": "test password",
    });
    let fresh_token = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority.system_token,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let export = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
                        "step_up_token": fresh_token,
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

    let delete_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.account.delete",
        &json!({ "account_id": account_id }),
    );
    let delete = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{account_id}"))
                .header("x-elastos-home-token", wallet_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "step_up_token": delete_token }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);

    let import = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
        "elastos.full-recovery-bundle.import.response/v2"
    );
    assert_eq!(import_json["wallet_restore"]["status"], "complete");
    assert_eq!(import_json["wallet_restore"]["expected_count"], 1);
    assert_eq!(import_json["wallet_restore"]["imported_count"], 1);
    assert_eq!(import_json["wallet_restore"]["reason_code"], "none");
    assert_eq!(import_json["runtime_audit"]["status"], "complete");
    assert_eq!(import_json["runtime_audit"]["reason_code"], "none");

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    assert_eq!(restored_account["label"], "Spending");

    let expected_system_authority = runtime_wallet_authority_for_app_token(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority.system_token,
    );
    let requests = wallet_provider.requests.lock().await;
    let requests = decode_recorded_wallet_requests(&requests);
    let export_request = requests
        .iter()
        .find(|request| request.operation.kind() == WalletOperationKind::ExportManagedRecoverySet)
        .expect("typed managed recovery-set export");
    let import_request = requests
        .iter()
        .find(|request| request.operation.kind() == WalletOperationKind::ImportManagedRecoverySet)
        .expect("typed managed recovery-set import");
    assert_wallet_authority(export_request, &expected_system_authority);
    assert_wallet_authority(import_request, &expected_system_authority);
    assert_eq!(
        import_request.operation.kind(),
        WalletOperationKind::ImportManagedRecoverySet
    );
}

#[tokio::test]
async fn test_full_recovery_activation_migrates_existing_plaintext_gba_save() {
    // First-run reality: the shell (and here a GBA save) writes declared
    // plaintext before a person can reach Recovery, while the offline
    // migration path demands protection that cannot exist yet. Activation
    // therefore migrates surviving plaintext under its own guard: the export
    // succeeds, the object becomes an envelope, and the original bytes land
    // in a backup under the Home's backups directory.
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let save_uri = format!(
        "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
        principal.localhost_root
    );
    let save_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &save_uri).unwrap();
    std::fs::create_dir_all(save_path.parent().unwrap()).unwrap();
    std::fs::write(&save_path, b"existing uCity save").unwrap();
    let export_intent = json!({
        "principal_id": authority.principal_id,
        "localhost_root": principal.localhost_root,
        "label": "Everything",
        "download_password": null,
    });
    let fresh_token = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority.system_token,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );

    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
                        "step_up_token": fresh_token,
                        "download_password": null
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let migrated = std::fs::read(&save_path).unwrap();
    assert_ne!(migrated, b"existing uCity save");
    let envelope: Value = serde_json::from_slice(&migrated).expect("migrated object is JSON");
    assert!(envelope["schema"]
        .as_str()
        .is_some_and(|schema| schema.starts_with("elastos.principal-root.object/")));

    assert!(crate::auth::load_principal_root_protection(
        dir.path(),
        &authority.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .is_some());

    let backups_root = dir.path().join("backups");
    let backup_entry = std::fs::read_dir(&backups_root)
        .expect("backups directory exists")
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("principal-root-migration-")
        })
        .expect("migration backup directory exists");
    let mut backed_up = Vec::new();
    let mut pending = vec![backup_entry.path()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(&path)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path()),
            );
        } else if std::fs::read(&path)
            .map(|bytes| bytes == b"existing uCity save")
            .unwrap_or(false)
        {
            backed_up.push(path);
        }
    }
    assert!(
        !backed_up.is_empty(),
        "backup must preserve the original plaintext bytes"
    );
}

#[tokio::test]
async fn test_full_recovery_fresh_export_rejects_envelope_shaped_data_before_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    let save_uri = format!(
        "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
        principal.localhost_root
    );
    let save_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &save_uri).unwrap();
    std::fs::create_dir_all(save_path.parent().unwrap()).unwrap();
    let envelope = json!({
        "schema": "elastos.principal-root.object/v1",
        "principal_id": authority.principal_id,
        "localhost_root": principal.localhost_root,
        "data_key_id": "pdek:unbound-envelope",
        "object_uri": save_uri,
        "cipher": "aes-256-gcm",
        "nonce": "AAAAAAAAAAAAAAAA",
        "ciphertext": "AQIDBAUGBwgJCgsMDQ4PEA"
    });
    let envelope_bytes = serde_json::to_vec_pretty(&envelope).unwrap();
    std::fs::write(&save_path, &envelope_bytes).unwrap();
    let export_intent = json!({
        "principal_id": authority.principal_id,
        "localhost_root": principal.localhost_root,
        "label": "Everything",
        "download_password": null,
    });
    let fresh_token = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority.system_token,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let auth_state_path = crate::auth::auth_state_path(dir.path()).unwrap();
    let auth_state_before = std::fs::read(&auth_state_path).unwrap();
    let archive_key_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), "ElastOS/System/Auth")
            .unwrap()
            .join("recovery-archive.key");
    assert!(!archive_key_path.exists());

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
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
                            "step_up_token": fresh_token,
                            "download_password": null
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
        assert!(String::from_utf8_lossy(&body)
            .contains("exact verified protection binding is required"));
    }

    assert_eq!(std::fs::read(&save_path).unwrap(), envelope_bytes);
    assert_eq!(std::fs::read(auth_state_path).unwrap(), auth_state_before);
    assert!(!archive_key_path.exists());
    assert!(crate::auth::load_principal_root_protection(
        dir.path(),
        &authority.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn test_full_recovery_bundle_recovers_existing_account_under_new_passkey() {
    let dir = tempfile::tempdir().unwrap();
    let (state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);
    let original = passkey_authority_with_name(dir.path(), Some("original"));
    let original_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &original.proof_binding_id)
            .unwrap();
    let original_wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &original);

    let create_account = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
        "download_password": "test password",
    });
    let fresh_token = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &original.system_token,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let export = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
                        "step_up_token": fresh_token,
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
    let replacement_session_id = replacement.session_id.clone();

    let import = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
    let import_status = import.status();
    let import_body = axum::body::to_bytes(import.into_body(), usize::MAX)
        .await
        .unwrap();
    let import_text = String::from_utf8(import_body.to_vec()).unwrap();
    assert_eq!(
        import_status,
        StatusCode::OK,
        "unexpected recovery response: {import_text}"
    );
    let import_json: serde_json::Value = serde_json::from_str(&import_text).unwrap();
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
    assert_eq!(import_json["wallet_restore"]["status"], "complete");
    assert_eq!(import_json["wallet_restore"]["expected_count"], 1);
    assert_eq!(import_json["wallet_restore"]["imported_count"], 1);
    assert_eq!(import_json["runtime_audit"]["status"], "complete");
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

    let replacement_system_token = import_json["system_token"].as_str().unwrap();
    let post_recovery_authority = runtime_wallet_authority_for_app_token(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        replacement_system_token,
    );
    assert_eq!(
        post_recovery_authority.verified_context().principal_id(),
        original_principal.principal_id
    );
    assert_eq!(
        post_recovery_authority
            .verified_context()
            .proof_binding_id(),
        Some(replacement.proof_binding_id.as_str())
    );
    assert_ne!(
        post_recovery_authority.verified_context().session_id(),
        replacement_session_id
    );
    let requests = wallet_provider.requests.lock().await;
    let requests = decode_recorded_wallet_requests(&requests);
    let import_request = requests
        .iter()
        .find(|request| request.operation.kind() == WalletOperationKind::ImportManagedRecoverySet)
        .expect("post-reassignment typed managed recovery-set import");
    assert_wallet_authority(import_request, &post_recovery_authority);
}

/// A Full Recovery Bundle carries the signed People identity, and importing
/// it on a genuinely fresh machine — a different data root with a different
/// device key — restores the same Profile DID and authorizes the new device
/// through the normal signed-revision path, reported honestly in the response.
#[tokio::test]
async fn test_full_recovery_bundle_restores_people_identity_on_a_fresh_machine() {
    assert_fresh_machine_full_recovery(false).await;
}

#[tokio::test]
async fn test_full_recovery_after_real_enrollment_preserves_single_profile() {
    assert_fresh_machine_full_recovery(true).await;
}

async fn enroll_recovering_device_with_current_flow(app: &axum::Router) -> Value {
    let request = |path: &str, cookie: &str, body: Value| {
        axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("host", "localhost:61180")
            .header("origin", "http://localhost:61180")
            .header("cookie", cookie)
            .header(CONTENT_TYPE, "application/json")
            .extension(axum::extract::ConnectInfo(
                "127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap(),
            ))
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let begin = app
        .clone()
        .oneshot(request(
            "/api/auth/passkey/register/begin",
            "",
            json!({"intent": {"purpose": "recover"}}),
        ))
        .await
        .unwrap();
    assert_eq!(begin.status(), StatusCode::OK);
    let cookie = begin.headers()[axum::http::header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let begin: Value = serde_json::from_slice(
        &axum::body::to_bytes(begin.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    let attestation = crate::auth::owner_attestation_for_test(
        begin["options"]["publicKey"]["challenge"].as_str().unwrap(),
        "localhost",
        "http://localhost:61180",
    );
    let complete = app
        .clone()
        .oneshot(request(
            "/api/auth/passkey/register/complete",
            &cookie,
            json!({
                "ceremony_id": begin["ceremony_id"],
                "response": attestation,
                "intent": {"purpose": "recover"}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(complete.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(complete.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn assert_fresh_machine_full_recovery(real_enrollment: bool) {
    let original_dir = tempfile::tempdir().unwrap();
    let (original_state, _original_provider) =
        wallet_chain_test_state_with_observer(original_dir.path()).await;
    let original_app = gateway_router(original_state);
    let original = passkey_authority_with_name(original_dir.path(), Some("original"));
    let original_principal = crate::auth::load_principal_for_proof_binding(
        original_dir.path(),
        &original.proof_binding_id,
    )
    .unwrap();
    let _ = elastos_identity::load_or_create_did(original_dir.path()).unwrap();
    let (status, _) = super::home_system::home_test_post_json(
        &original_app,
        "/api/apps/people/profile",
        &original.people_token,
        "null",
        json!({"display_name": "Original Person"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let export_intent = json!({
        "principal_id": original.principal_id,
        "localhost_root": original_principal.localhost_root,
        "label": "Everything",
        "download_password": "test password",
    });
    let fresh_token = step_up_token_for_app_context(
        original_dir.path(),
        SYSTEM_CAPSULE_ID,
        &original.system_token,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let export = original_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
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
                        "step_up_token": fresh_token,
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
    let saved = crate::collaboration_profile_authority::load_profile_authority(
        original_dir.path(),
        &original.principal_id,
        &original_principal.localhost_root,
    )
    .unwrap()
    .expect("first setup exports its new Profile");
    let profile_did = saved.document().profile_did.clone();
    assert_eq!(saved.document().display_name, "Original Person");
    assert_eq!(saved.document().revision, 1);

    // The fresh machine: a different data root whose device key cannot be the
    // original's.
    let fresh_dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fresh_dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (fresh_state, _fresh_provider) =
        wallet_chain_test_state_with_observer(fresh_dir.path()).await;
    let fresh_app = gateway_router(fresh_state);
    let replacement = if real_enrollment {
        enroll_recovering_device_with_current_flow(&fresh_app).await
    } else {
        let authority = passkey_authority_with_name(fresh_dir.path(), Some("replacement"));
        json!({
            "principal_id": authority.principal_id,
            "proof_binding_id": authority.proof_binding_id,
            "system_token": authority.system_token
        })
    };
    let replacement_principal = crate::auth::load_principal_for_proof_binding(
        fresh_dir.path(),
        replacement["proof_binding_id"].as_str().unwrap(),
    )
    .unwrap();
    let replacement_profile = crate::collaboration_profile_authority::load_profile_authority(
        fresh_dir.path(),
        &replacement_principal.principal_id,
        &replacement_principal.localhost_root,
    )
    .unwrap();

    let import = fresh_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/auth/recovery/full-import")
                .header(
                    "x-elastos-home-token",
                    replacement["system_token"].as_str().unwrap(),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.import.request/v1",
                        "principal_id": replacement["principal_id"],
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
    let import_status = import.status();
    let import_body = axum::body::to_bytes(import.into_body(), usize::MAX)
        .await
        .unwrap();
    let import_text = String::from_utf8(import_body.to_vec()).unwrap();
    assert_eq!(
        import_status,
        StatusCode::OK,
        "unexpected recovery response: {import_text}"
    );
    let import_json: serde_json::Value = serde_json::from_str(&import_text).unwrap();
    assert_eq!(import_json["status"], "reassigned");
    let people = &import_json["people_identity_restore"];
    assert_eq!(
        people["status"], "restored",
        "People restore status={} reason={}",
        people["status"], people["reason"]
    );
    assert_eq!(people["profile_did"], profile_did);
    assert_eq!(people["rebound_device"], true);
    assert_eq!(people["contact_store_restored"], false);

    // The recovered head on the fresh machine is the next signed revision,
    // keeps the person's name, and authorizes exactly this machine's device.
    let recovered_head = crate::collaboration_profile_authority::load_profile_authority(
        fresh_dir.path(),
        &original_principal.principal_id,
        &original_principal.localhost_root,
    )
    .unwrap()
    .expect("recovered profile authority present");
    assert_eq!(recovered_head.document().profile_did, profile_did);
    assert_eq!(
        recovered_head.document().revision,
        saved.document().revision + 1
    );
    assert_eq!(recovered_head.document().display_name, "Original Person");
    let summary = fresh_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header(
                    "x-elastos-home-token",
                    import_json["home_token"].as_str().unwrap(),
                )
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
    assert_eq!(summary_json["authority"]["signed_in"], true);
    assert_eq!(
        summary_json["identity"]["profile_readiness"]["status"],
        "ready"
    );
    assert_eq!(
        summary_json["identity"]["profile"]["display_name"],
        "Original Person"
    );
    let (_, fresh_device_did) = elastos_identity::load_or_create_did(fresh_dir.path()).unwrap();
    assert!(recovered_head.authorizes_endpoint(&fresh_device_did));
    assert!(
        replacement_profile.is_none(),
        "recovery entry created an extra public Profile before restoring the original identity"
    );
}

struct MalformedFullRecoveryWalletProvider;

#[async_trait::async_trait]
impl Provider for MalformedFullRecoveryWalletProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "malformed recovery test provider supports only raw requests".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["wallet"]
    }

    fn name(&self) -> &'static str {
        "malformed-full-recovery-wallet-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(
            request.get("op").and_then(Value::as_str),
            Some(WALLET_BUS_OPERATION)
        );
        let wallet_request = WalletProviderRequestV2::decode_at(
            &serde_json::to_vec(request.get("request").unwrap()).unwrap(),
            crate::auth::now_ts(),
        )
        .unwrap();
        Ok(json!({
            "status": "ok",
            "data": WalletProviderResponseV2::for_request(
                &wallet_request,
                WalletResultV2::Ok {
                    data: json!({
                        "imported": true,
                        "account_count": 1,
                        "accounts": [],
                    }),
                },
            ),
        }))
    }
}

#[tokio::test]
async fn test_full_recovery_bundle_returns_committed_root_and_retries_wallet_restore() {
    let dir = tempfile::tempdir().unwrap();
    let export_app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let original = passkey_authority_with_name(dir.path(), Some("original"));
    let original_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &original.proof_binding_id)
            .unwrap();
    let bundle = create_and_export_raw_full_recovery_bundle(
        &export_app,
        dir.path(),
        &original,
        &original_principal,
    )
    .await;
    assert_eq!(bundle["schema"], "elastos.full-recovery-bundle/v1");
    assert_eq!(bundle["wallet_recovery_keys"].as_array().unwrap().len(), 1);

    let replacement = passkey_authority_with_name_role(
        dir.path(),
        Some("replacement"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let replacement_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &replacement.proof_binding_id)
            .unwrap();
    let unavailable_app = gateway_router(test_state(dir.path()));
    let (status, incomplete) = import_raw_full_recovery_bundle(
        &unavailable_app,
        &replacement.system_token,
        &replacement_principal.principal_id,
        &replacement_principal.localhost_root,
        &bundle,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{incomplete}");
    assert_eq!(
        incomplete["schema"],
        "elastos.full-recovery-bundle.import.response/v2"
    );
    assert_eq!(incomplete["status"], "reassigned");
    assert_eq!(incomplete["wallet_restore"]["status"], "incomplete");
    assert_eq!(incomplete["wallet_restore"]["expected_count"], 1);
    assert_eq!(incomplete["wallet_restore"]["imported_count"], 0);
    assert_eq!(
        incomplete["wallet_restore"]["reason_code"],
        "wallet_provider_unavailable"
    );
    assert_eq!(incomplete["runtime_audit"]["status"], "complete");
    assert!(!incomplete.to_string().contains("private_key_hex"));
    let home_token = incomplete["home_token"].as_str().unwrap().to_string();
    let system_token = incomplete["system_token"].as_str().unwrap().to_string();

    let retry_app = gateway_router(wallet_chain_test_state(dir.path()).await);
    for (token, origin) in [
        (&home_token, "http://localhost:61180"),
        (&system_token, "null"),
    ] {
        let usable = retry_app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", origin)
                    .uri("/api/auth/recovery/status")
                    .header("x-elastos-home-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(usable.status(), StatusCode::OK);
    }
    let old_token = retry_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/auth/recovery/status")
                .header("x-elastos-home-token", replacement.system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            old_token.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "old pre-reassignment token remained usable: {}",
        old_token.status()
    );

    let (retry_status, complete) = import_raw_full_recovery_bundle(
        &retry_app,
        &system_token,
        incomplete["principal_id"].as_str().unwrap(),
        incomplete["localhost_root"].as_str().unwrap(),
        &bundle,
        false,
    )
    .await;
    assert_eq!(retry_status, StatusCode::OK, "{complete}");
    assert_eq!(complete["status"], "imported");
    assert_eq!(complete["wallet_restore"]["status"], "complete");
    assert_eq!(complete["wallet_restore"]["expected_count"], 1);
    assert_eq!(complete["wallet_restore"]["imported_count"], 1);
    assert_eq!(complete["wallet_restore"]["reason_code"], "none");
    assert_eq!(complete["runtime_audit"]["status"], "complete");

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let incomplete_audit = auth_state
        .audit
        .iter()
        .find(|event| event.event_type == "auth.full_recovery_bundle.import_incomplete")
        .expect("incomplete Wallet restore audit");
    assert_eq!(incomplete_audit.result, "incomplete");
    assert!(incomplete_audit
        .reason
        .contains("reason_code=wallet_provider_unavailable"));
    assert!(incomplete_audit.reason.contains("expected_count=1"));
    assert!(incomplete_audit.reason.contains("imported_count=0"));
    let complete_audit = auth_state
        .audit
        .iter()
        .rev()
        .find(|event| event.event_type == "auth.full_recovery_bundle.imported")
        .expect("complete Wallet restore audit");
    assert_eq!(complete_audit.result, "ok");
    assert!(complete_audit.reason.contains("expected_count=1"));
    assert!(complete_audit.reason.contains("imported_count=1"));
    let audit_json = serde_json::to_string(&auth_state.audit).unwrap();
    assert!(!audit_json.contains("private_key_hex"));
    assert!(
        !audit_json.contains("1111111111111111111111111111111111111111111111111111111111111111")
    );
}

#[tokio::test]
async fn test_full_recovery_bundle_returns_tokens_when_outcome_audit_needs_retry() {
    let dir = tempfile::tempdir().unwrap();
    let export_app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let original = passkey_authority_with_name(dir.path(), Some("original"));
    let original_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &original.proof_binding_id)
            .unwrap();
    let bundle = create_and_export_raw_full_recovery_bundle(
        &export_app,
        dir.path(),
        &original,
        &original_principal,
    )
    .await;
    let replacement = passkey_authority_with_name_role(
        dir.path(),
        Some("replacement"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let replacement_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &replacement.proof_binding_id)
            .unwrap();
    let (import_state, wallet_provider) = wallet_chain_test_state_with_observer(dir.path()).await;
    let import_app = gateway_router(import_state);
    crate::auth::inject_recovery_reassignment_test_fault(
        dir.path(),
        crate::auth::RecoveryReassignmentTestFault::PostCommitOutcomeAudit,
    );

    let (status, incomplete) = import_raw_full_recovery_bundle(
        &import_app,
        &replacement.system_token,
        &replacement_principal.principal_id,
        &replacement_principal.localhost_root,
        &bundle,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{incomplete}");
    assert_eq!(incomplete["status"], "reassigned");
    assert_eq!(incomplete["wallet_restore"]["status"], "complete");
    assert_eq!(incomplete["runtime_audit"]["status"], "incomplete");
    assert_eq!(
        incomplete["runtime_audit"]["reason_code"],
        "runtime_audit_unavailable"
    );
    let terminal_retry_token = incomplete["runtime_audit"]["retry_token"]
        .as_str()
        .expect("signed non-secret terminal retry token")
        .to_string();
    assert!(!incomplete.to_string().contains("private_key_hex"));
    let home_token = incomplete["home_token"].as_str().unwrap();
    let system_token = incomplete["system_token"].as_str().unwrap();
    for (token, origin) in [
        (home_token, "http://localhost:61180"),
        (system_token, "null"),
    ] {
        let usable = import_app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", origin)
                    .uri("/api/auth/recovery/status")
                    .header("x-elastos-home-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(usable.status(), StatusCode::OK);
    }
    let old_session = import_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/auth/recovery/status")
                .header("x-elastos-home-token", replacement.system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            old_session.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "old pre-reassignment token remained usable: {}",
        old_session.status()
    );

    let mut same_id_same_count_substitution = bundle.clone();
    same_id_same_count_substitution["created_at"] =
        json!(bundle["created_at"].as_u64().unwrap() + 1);
    let mut kit_substitution = bundle.clone();
    kit_substitution["data_kit"]["instructions"][0] =
        json!("Substituted Recovery Kit instructions.");
    let mut wallet_key_substitution = bundle.clone();
    wallet_key_substitution["wallet_recovery_keys"][0]["private_key_hex"] =
        json!("2222222222222222222222222222222222222222222222222222222222222222");
    for (case, substituted) in [
        ("same-id same-count bundle", same_id_same_count_substitution),
        ("Recovery Kit", kit_substitution),
        ("Wallet key", wallet_key_substitution),
    ] {
        let (substitution_status, substitution_response) =
            import_raw_full_recovery_bundle_with_terminal_retry(
                &import_app,
                system_token,
                incomplete["principal_id"].as_str().unwrap(),
                incomplete["localhost_root"].as_str().unwrap(),
                &substituted,
                false,
                Some(&terminal_retry_token),
            )
            .await;
        assert_eq!(
            substitution_status,
            StatusCode::FORBIDDEN,
            "{case} substitution reused terminal evidence: {substitution_response}"
        );
    }

    let now = crate::auth::now_ts();
    let cross_session_grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", uuid_like_token()),
        session_id: format!("auth:{}", uuid_like_token()),
        principal_id: incomplete["principal_id"].as_str().unwrap().to_string(),
        proof_binding_id: replacement.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec![SYSTEM_CAPSULE_ID.to_string()],
    };
    crate::auth::store_session_grant(dir.path(), cross_session_grant.clone()).unwrap();
    let cross_session_token =
        issue_home_launch_token_for_auth_grant(dir.path(), SYSTEM_CAPSULE_ID, &cross_session_grant)
            .unwrap();
    let (cross_session_status, cross_session_response) =
        import_raw_full_recovery_bundle_with_terminal_retry(
            &import_app,
            &cross_session_token,
            incomplete["principal_id"].as_str().unwrap(),
            incomplete["localhost_root"].as_str().unwrap(),
            &bundle,
            false,
            Some(&terminal_retry_token),
        )
        .await;
    assert_eq!(
        cross_session_status,
        StatusCode::FORBIDDEN,
        "cross-session terminal replay succeeded: {cross_session_response}"
    );

    let (retry_status, complete) = import_raw_full_recovery_bundle_with_terminal_retry(
        &import_app,
        system_token,
        incomplete["principal_id"].as_str().unwrap(),
        incomplete["localhost_root"].as_str().unwrap(),
        &bundle,
        false,
        Some(&terminal_retry_token),
    )
    .await;
    assert_eq!(retry_status, StatusCode::OK, "{complete}");
    assert_eq!(complete["wallet_restore"]["status"], "complete");
    assert_eq!(complete["runtime_audit"]["status"], "complete");
    assert_eq!(complete["runtime_audit"]["reason_code"], "none");

    let (repeat_status, repeated) = import_raw_full_recovery_bundle(
        &import_app,
        system_token,
        incomplete["principal_id"].as_str().unwrap(),
        incomplete["localhost_root"].as_str().unwrap(),
        &bundle,
        false,
    )
    .await;
    assert_eq!(repeat_status, StatusCode::OK, "{repeated}");
    assert_eq!(repeated["wallet_restore"]["status"], "complete");
    assert_eq!(repeated["runtime_audit"]["status"], "complete");

    let (new_session_status, new_session_repeat) = import_raw_full_recovery_bundle(
        &import_app,
        &cross_session_token,
        incomplete["principal_id"].as_str().unwrap(),
        incomplete["localhost_root"].as_str().unwrap(),
        &bundle,
        false,
    )
    .await;
    assert_eq!(new_session_status, StatusCode::OK, "{new_session_repeat}");
    assert_eq!(new_session_repeat["runtime_audit"]["status"], "complete");

    let (old_retry_status, _) = import_raw_full_recovery_bundle_with_terminal_retry(
        &import_app,
        &cross_session_token,
        incomplete["principal_id"].as_str().unwrap(),
        incomplete["localhost_root"].as_str().unwrap(),
        &bundle,
        false,
        Some(&terminal_retry_token),
    )
    .await;
    assert_eq!(old_retry_status, StatusCode::FORBIDDEN);

    let unrelated = passkey_authority_with_name_role(
        dir.path(),
        Some("unrelated"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    for (token, root) in [
        (
            unrelated.system_token.as_str(),
            incomplete["localhost_root"].as_str().unwrap(),
        ),
        (cross_session_token.as_str(), "localhost://wrong-root"),
    ] {
        let (status, response) = import_raw_full_recovery_bundle(
            &import_app,
            token,
            incomplete["principal_id"].as_str().unwrap(),
            root,
            &bundle,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{response}");
    }

    let requests = wallet_provider.requests.lock().await;
    let import_count = decode_recorded_wallet_requests(&requests)
        .iter()
        .filter(|request| request.operation.kind() == WalletOperationKind::ImportManagedRecoverySet)
        .count();
    assert_eq!(
        import_count, 1,
        "terminal retry repeated a completed Wallet import"
    );
    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    assert_eq!(
        auth_state
            .audit
            .iter()
            .filter(|event| event.event_type == "auth.recovery_kit.reassigned")
            .count(),
        1
    );
    assert_eq!(
        auth_state
            .audit
            .iter()
            .filter(|event| event.event_type == "auth.full_recovery_bundle.imported")
            .count(),
        1
    );
    assert!(!serde_json::to_string(&auth_state.audit)
        .unwrap()
        .contains("private_key_hex"));
}

#[tokio::test]
async fn test_full_recovery_bundle_sanitizes_malformed_wallet_restore_after_reassignment() {
    let dir = tempfile::tempdir().unwrap();
    let export_app = gateway_router(wallet_chain_test_state(dir.path()).await);
    let original = passkey_authority_with_name(dir.path(), Some("original"));
    let original_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &original.proof_binding_id)
            .unwrap();
    let bundle = create_and_export_raw_full_recovery_bundle(
        &export_app,
        dir.path(),
        &original,
        &original_principal,
    )
    .await;
    let replacement = passkey_authority_with_name_role(
        dir.path(),
        Some("replacement"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let replacement_principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &replacement.proof_binding_id)
            .unwrap();
    let malformed_state = wallet_chain_test_state_with_shared_wallet_provider(
        dir.path(),
        Arc::new(MalformedFullRecoveryWalletProvider),
    )
    .await;
    let malformed_app = gateway_router(malformed_state);
    let (status, response) = import_raw_full_recovery_bundle(
        &malformed_app,
        &replacement.system_token,
        &replacement_principal.principal_id,
        &replacement_principal.localhost_root,
        &bundle,
        true,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["status"], "reassigned");
    assert_eq!(response["wallet_restore"]["status"], "incomplete");
    assert_eq!(response["wallet_restore"]["expected_count"], 1);
    assert_eq!(response["wallet_restore"]["imported_count"], 0);
    assert_eq!(
        response["wallet_restore"]["reason_code"],
        "wallet_provider_invalid_response"
    );
    assert_eq!(response["runtime_audit"]["status"], "complete");
    assert!(response["home_token"]
        .as_str()
        .is_some_and(|token| !token.is_empty()));
    assert!(response["system_token"]
        .as_str()
        .is_some_and(|token| !token.is_empty()));
    let serialized = response.to_string();
    assert!(!serialized.contains("missing status"));
    assert!(!serialized.contains("invalid Wallet provider"));
    assert!(!serialized.contains("private_key_hex"));
}
