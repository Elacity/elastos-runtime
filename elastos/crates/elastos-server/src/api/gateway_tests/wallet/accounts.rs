use super::super::*;

#[test]
fn raw_wallet_account_dispatch_is_absent_from_production_routes() {
    fn collect_raw_dispatches(
        api_dir: &std::path::Path,
        dir: &std::path::Path,
        matches: &mut Vec<(String, String, usize)>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) != Some("gateway_tests") {
                    collect_raw_dispatches(api_dir, &path, matches);
                }
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
                || file_name.ends_with("_tests.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for operation in [
                "accounts",
                "create_managed_account",
                "revoke_account",
                "rename_account",
                "set_default_account",
                "export_managed_secret",
                "import_managed_secret",
            ] {
                let count = source.matches(&format!(r#""op": "{operation}""#)).count();
                if count > 0 {
                    matches.push((
                        path.strip_prefix(api_dir)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        operation.to_string(),
                        count,
                    ));
                }
            }
        }
    }

    let api_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
    let mut matches = Vec::new();
    collect_raw_dispatches(&api_dir, &api_dir, &mut matches);
    matches.sort();
    assert_eq!(
        matches,
        Vec::<(String, String, usize)>::new(),
        "raw Wallet account or managed Recovery Key dispatch remains in a production route"
    );

    let recovery_bundle = std::fs::read_to_string(api_dir.join("auth_gateway.rs")).unwrap();
    assert!(recovery_bundle.contains("ExportManagedRecoverySet"));
    assert!(recovery_bundle.contains("ImportManagedRecoverySet"));
    assert!(recovery_bundle.contains("ManagedRecoverySetV1"));
    assert!(!recovery_bundle.contains("wallet_recovery_keys_for_principal"));
    assert!(!recovery_bundle.contains(r#""op": "accounts""#));
    assert!(!recovery_bundle.contains(r#""op": "export_managed_secret""#));
    assert!(!recovery_bundle.contains(r#""op": "import_managed_secret""#));

    let translation_start = recovery_bundle
        .find("fn full_bundle_wallet_recovery_keys")
        .unwrap();
    let translation_end = recovery_bundle
        .find("fn replacement_wallet_authority")
        .unwrap();
    let translation = &recovery_bundle[translation_start..translation_end];
    assert!(translation.contains("account_id"));
    assert!(translation.contains("label"));
    for provider_owned_semantic in [
        "proof_type",
        "private_key",
        "secret_type",
        "chain_namespace",
        "address",
    ] {
        assert!(
            !translation.contains(provider_owned_semantic),
            "Full Recovery translation inspected Provider-owned {provider_owned_semantic}"
        );
    }
}

#[test]
fn managed_recovery_key_routes_are_authority_bound_wallet_bus_v2_only() {
    let source = include_str!("../../gateway_wallet_app.rs");
    let export_start = source
        .find("async fn wallet_app_account_recovery_key")
        .unwrap();
    let import_start = source
        .find("async fn wallet_app_account_import_recovery_key")
        .unwrap();
    let next_route = source[import_start + 1..]
        .find("\npub(in crate::api::gateway) async fn ")
        .map(|offset| import_start + 1 + offset)
        .unwrap();
    let routes = [
        (
            &source[export_start..import_start],
            "ExportManagedRecoveryKey",
        ),
        (
            &source[import_start..next_route],
            "ImportManagedRecoveryKey",
        ),
    ];

    for (route, operation) in routes {
        assert!(route.contains("runtime_wallet_data("));
        assert!(route.contains(operation));
        assert!(!route.contains("wallet_provider_data("));
        assert!(!route.contains(r#""op": "export_managed_secret""#));
        assert!(!route.contains(r#""op": "import_managed_secret""#));

        let dispatch_start = route.find("match runtime_wallet_data(").unwrap();
        let audit_start = route.find("append_wallet_approval_audit(").unwrap();
        let dispatch = &route[dispatch_start..audit_start];
        assert!(
            !dispatch.contains("principal_id"),
            "{operation} dispatch supplied principal_id outside Runtime authority"
        );
    }
}

struct MalformedRecoveryWalletProvider;

#[async_trait::async_trait]
impl Provider for MalformedRecoveryWalletProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "malformed test Wallet supports only raw requests".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["wallet"]
    }

    fn name(&self) -> &'static str {
        "malformed-recovery-wallet-provider"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(request["op"], WALLET_BUS_OPERATION);
        Ok(json!({"status": "ok", "data": {}}))
    }
}

#[tokio::test]
async fn test_system_token_can_review_and_reject_wallet_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), SYSTEM_CAPSULE_ID, &context).unwrap();
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::default(),
        approvals: TokioMutex::new(vec![json!({
            "request_id": "wallet-approval:test",
            "status": "pending",
            "intent": "publish_envelope",
            "capsule_id": "documents",
            "resource": "elastos://content/publish",
            "reason": "Publish document revision",
            "account_id": "wallet:eip155:20:0xabc",
            "address": "0xabc",
            "principal_id": context.principal_id.clone(),
            "created_at": 10,
            "expires_at": 20
        })]),
        defaults: TokioMutex::default(),
    };
    let app = gateway_router(wallet_test_state_with_provider(dir.path(), provider).await);

    let approvals = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/system/wallet/approvals")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approvals.status(), StatusCode::OK);
    let approvals_body = axum::body::to_bytes(approvals.into_body(), usize::MAX)
        .await
        .unwrap();
    let approvals_json: serde_json::Value = serde_json::from_slice(&approvals_body).unwrap();
    assert_eq!(approvals_json["available"], true);
    assert_eq!(approvals_json["pending_count"], 1);
    assert_eq!(
        approvals_json["approval_requests"][0]["intent"],
        "publish_envelope"
    );

    let rejected = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/approvals/wallet-approval%3Atest/reject")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"reason":"Not now"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::OK);
    let rejected_body = axum::body::to_bytes(rejected.into_body(), usize::MAX)
        .await
        .unwrap();
    let rejected_json: serde_json::Value = serde_json::from_slice(&rejected_body).unwrap();
    assert_eq!(rejected_json["pending_count"], 0);
    assert!(rejected_json["approval_requests"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_system_can_create_managed_wallet_account() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), SYSTEM_CAPSULE_ID, &context).unwrap();
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), SYSTEM_CAPSULE_ID, &token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/wallet/managed")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["available"], true);
    assert_eq!(json["linked_count"], 3);
    let accounts = json["accounts"].as_array().unwrap();
    let namespaces = accounts
        .iter()
        .map(|account| account["chain_namespace"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        namespaces,
        BTreeSet::from([
            "eip155:20",
            "eip155:8453",
            "bip122:000000000019d6689c085ae165831e93"
        ])
    );
    let evm_addresses = accounts
        .iter()
        .filter(|account| account["proof_type"] == "managed_evm")
        .map(|account| account["address"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(evm_addresses.len(), 1);
    let bitcoin = accounts
        .iter()
        .find(|account| account["proof_type"] == "managed_btc_p2wpkh")
        .expect("Bitcoin managed account");
    assert_eq!(
        bitcoin["chain_namespace"],
        "bip122:000000000019d6689c085ae165831e93"
    );
    assert!(bitcoin["address"].as_str().unwrap().starts_with("bc1q"));
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
}

#[tokio::test]
async fn test_wallet_app_can_create_and_summarize_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let token =
        issue_home_launch_token_with_context(dir.path(), WALLET_CAPSULE_ID, &context).unwrap();
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);

    let extra = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Agent Budget","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(extra.status(), StatusCode::OK);

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["app"]["title"], "Wallet");
    assert_eq!(
        json["approval_methods"]["walletconnect"]["available"],
        false
    );
    assert_eq!(
        json["approval_methods"]["walletconnect"]["requires_pinned_config"],
        true
    );
    assert_eq!(json["wallet_accounts"]["linked_count"], 4);
    assert_eq!(json["wallet_approvals"]["pending_count"], 0);
    let accounts = json["wallet_accounts"]["accounts"].as_array().unwrap();
    assert!(accounts
        .iter()
        .any(|account| account["label"] == "Spending"));
    assert!(accounts
        .iter()
        .any(|account| account["label"] == "ELA Wallet"));
    assert!(accounts.iter().any(|account| account["label"] == "Savings"));
    assert!(accounts
        .iter()
        .any(|account| account["label"] == "Agent Budget"));
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
}

#[tokio::test]
async fn test_wallet_summary_preserves_unavailable_provider_schema() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let app = gateway_router(test_state(dir.path()));

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let accounts = &payload["wallet_accounts"];
    assert_eq!(accounts["available"], false);
    assert_eq!(accounts["linked_count"], 0);
    assert_eq!(accounts["accounts"], json!([]));
    assert_eq!(accounts["default_accounts"], json!([]));
    assert!(accounts["note"]
        .as_str()
        .unwrap()
        .contains("wallet provider unavailable"));
}

#[tokio::test]
async fn test_wallet_transaction_default_also_drives_browser_connect_default() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:20","label":"Spending","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let created_json: serde_json::Value = serde_json::from_slice(&created_body).unwrap();
    let account_id = created_json["accounts"][0]["account_id"].as_str().unwrap();

    let updated = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/default")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"account_id":"{account_id}","chain_namespace":"eip155:20","intent":"transaction_intent"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = axum::body::to_bytes(updated.into_body(), usize::MAX)
        .await
        .unwrap();
    let updated_json: serde_json::Value = serde_json::from_slice(&updated_body).unwrap();
    let defaults = updated_json["default_accounts"].as_array().unwrap();
    assert!(defaults.iter().any(|item| {
        item["account_id"] == account_id && item["intent"] == "transaction_intent"
    }));
    assert!(defaults
        .iter()
        .any(|item| { item["account_id"] == account_id && item["intent"] == "browser_connect" }));
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::SetDefaultAccount,
                WalletOperationKind::SetDefaultAccount,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
}

#[test]
fn system_wallet_account_summary_defaults_missing_signing_availability_to_false() {
    let summary = system_wallet_account_summary(&json!({
        "account_id": "wallet:eip155:20:0xd2feb944c17ebbe1048d8afdac997b3660d6375d",
        "chain_namespace": "eip155:20",
        "address": "0xd2feb944c17ebbe1048d8afdac997b3660d6375d",
        "proof_type": "managed_evm",
        "linked_at": 1770000000
    }))
    .unwrap();

    assert!(!summary.signing_available);
}

#[tokio::test]
async fn test_wallet_app_can_delete_managed_account() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Temporary","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account_id = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["label"] == "Temporary")
        .and_then(|account| account["account_id"].as_str())
        .unwrap()
        .to_string();
    let encoded = account_id.replace(':', "%3A");

    let missing_fresh = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"step_up_token":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_fresh.status(), StatusCode::FORBIDDEN);

    let delete_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &token,
        "wallet.account.delete",
        &json!({ "account_id": account_id }),
    );
    let deleted = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("DELETE")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"step_up_token":"{}"}}"#,
                    delete_token
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted_body = axum::body::to_bytes(deleted.into_body(), usize::MAX)
        .await
        .unwrap();
    let deleted_payload: serde_json::Value = serde_json::from_slice(&deleted_body).unwrap();
    assert_eq!(deleted_payload["linked_count"], 0);

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/wallet/wallet/summary")
                .header("x-elastos-home-token", token)
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
    assert_eq!(summary_payload["wallet_accounts"]["linked_count"], 0);
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::RevokeAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
}

#[tokio::test]
async fn test_wallet_app_can_rename_account() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Spending","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account_id = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["label"] == "Spending")
        .and_then(|account| account["account_id"].as_str())
        .unwrap()
        .to_string();
    let encoded = account_id.replace(':', "%3A");

    let renamed = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("PUT")
                .uri(format!("/api/apps/wallet/wallet/accounts/{encoded}"))
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"label":"Savings"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    let renamed_body = axum::body::to_bytes(renamed.into_body(), usize::MAX)
        .await
        .unwrap();
    let renamed_payload: serde_json::Value = serde_json::from_slice(&renamed_body).unwrap();
    assert!(renamed_payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|account| account["label"] == "Savings"));
    assert!(!renamed_payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|account| account["label"] == "Spending"));
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::RenameAccount,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
}

#[tokio::test]
async fn test_wallet_recovery_key_requires_passkey_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &wallet_token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let created = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/managed")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"chain_namespace":"eip155:8453","label":"Spending","create_new":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let account_id = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["label"] == "Spending")
        .and_then(|account| account["account_id"].as_str())
        .unwrap();
    let encoded = account_id.replace(':', "%3A");

    let rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": home_app_token(dir.path()),
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let fresh_authority = passkey_authority(dir.path());
    let fresh_wallet_token =
        app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &fresh_authority);
    let mismatched_export_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &fresh_wallet_token,
        "wallet.recovery-key.export",
        &json!({ "account_id": account_id }),
    );
    let mismatched = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": mismatched_export_token,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatched.status(), StatusCode::FORBIDDEN);

    let export_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.recovery-key.export",
        &json!({ "account_id": account_id }),
    );
    let exported = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": export_token.clone(),
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(exported.status(), StatusCode::OK);
    let exported_body = axum::body::to_bytes(exported.into_body(), usize::MAX)
        .await
        .unwrap();
    let exported_payload: serde_json::Value = serde_json::from_slice(&exported_body).unwrap();
    assert_eq!(exported_payload["schema"], "elastos.wallet.recovery-key/v1");
    assert_eq!(exported_payload["secret_type"], "secp256k1_private_key_hex");
    assert_eq!(
        exported_payload["private_key_hex"].as_str().unwrap().len(),
        64
    );

    let replayed = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!(
                    "/api/apps/wallet/wallet/accounts/{encoded}/recovery-key"
                ))
                .header("x-elastos-home-token", wallet_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": export_token,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replayed.status(), StatusCode::FORBIDDEN);
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::CreateManagedAccount,
                WalletOperationKind::ListAccounts,
                WalletOperationKind::ExportManagedRecoveryKey,
            ],
        )
        .await;
}

#[tokio::test]
async fn test_wallet_recovery_key_import_requires_passkey_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let wallet_read_authority =
        runtime_wallet_authority_for_app_token(dir.path(), WALLET_CAPSULE_ID, &wallet_token);
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let recovery_key = json!({
        "schema": "elastos.wallet.recovery-key/v1",
        "account_id": "wallet:eip155:8453:0x1111111111111111111111111111111111111111",
        "chain_namespace": "eip155:8453",
        "address": "0x1111111111111111111111111111111111111111",
        "secret_type": "secp256k1_private_key_hex",
        "private_key_hex": "1111111111111111111111111111111111111111111111111111111111111111",
    });

    let rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": home_app_token(dir.path()),
                        "recovery_key": recovery_key.clone(),
                        "label": "Recovered Base",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let import_intent = json!({
        "recovery_key": recovery_key,
        "label": "Recovered Base",
    });
    let import_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.recovery-key.import",
        &import_intent,
    );
    let imported = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": import_token,
                        "recovery_key": import_intent["recovery_key"],
                        "label": import_intent["label"],
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
    let body = axum::body::to_bytes(imported.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|account| {
            account["label"] == "Recovered Base"
                && account["signing_status"] == "managed_key_available"
        }));
    wallet_provider
        .assert_v2_account_operations(
            &wallet_read_authority,
            &[
                WalletOperationKind::ImportManagedRecoveryKey,
                WalletOperationKind::ListAccounts,
            ],
        )
        .await;
}

#[tokio::test]
async fn test_managed_recovery_key_routes_reject_unverified_or_mismatched_intent_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let wallet_token = app_token_for_authority(dir.path(), WALLET_CAPSULE_ID, &authority);
    let export_intent = json!({ "account_id": "account:expected" });
    let export_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.recovery-key.export",
        &export_intent,
    );
    let (state, wallet_provider) = wallet_test_state_with_observer(dir.path()).await;
    let app = gateway_router(state);

    let wrong_actor = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/account%3Aexpected/recovery-key")
                .header("x-elastos-home-token", authority.system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": export_token.clone(),
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_actor.status(), StatusCode::FORBIDDEN);

    let substituted_account = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/account%3Asubstituted/recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": export_token,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(substituted_account.status(), StatusCode::FORBIDDEN);

    let recovery_key = json!({"opaque": "provider-owned-recovery-material"});
    let import_intent = json!({
        "recovery_key": recovery_key,
        "label": "Expected label",
    });
    let import_token = step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.recovery-key.import",
        &import_intent,
    );
    let substituted_payload = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": import_token,
                        "recovery_key": import_intent["recovery_key"],
                        "label": "Substituted label",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(substituted_payload.status(), StatusCode::FORBIDDEN);

    let stale_token = stale_step_up_token_for_app_context(
        dir.path(),
        WALLET_CAPSULE_ID,
        &wallet_token,
        "wallet.recovery-key.import",
        &import_intent,
    );
    let stale = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": stale_token,
                        "recovery_key": import_intent["recovery_key"],
                        "label": import_intent["label"],
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::FORBIDDEN);

    let caller_authority = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", wallet_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": step_up_token_for_app_context(
                            dir.path(),
                            WALLET_CAPSULE_ID,
                            &wallet_token,
                            "wallet.recovery-key.import",
                            &import_intent,
                        ),
                        "recovery_key": import_intent["recovery_key"],
                        "label": import_intent["label"],
                        "principal_id": "principal:attacker",
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(caller_authority.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(wallet_provider.requests.lock().await.is_empty());
}

#[tokio::test]
async fn test_managed_recovery_key_routes_fail_closed_on_provider_errors() {
    let unavailable_dir = tempfile::tempdir().unwrap();
    let unavailable_authority = passkey_authority(unavailable_dir.path());
    let unavailable_token = app_token_for_authority(
        unavailable_dir.path(),
        WALLET_CAPSULE_ID,
        &unavailable_authority,
    );
    let unavailable_step_up = step_up_token_for_app_context(
        unavailable_dir.path(),
        WALLET_CAPSULE_ID,
        &unavailable_token,
        "wallet.recovery-key.export",
        &json!({ "account_id": "account:unavailable" }),
    );
    let unavailable = gateway_router(test_state(unavailable_dir.path()))
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/account%3Aunavailable/recovery-key")
                .header("x-elastos-home-token", unavailable_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": unavailable_step_up,
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let unavailable_body = axum::body::to_bytes(unavailable.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8(unavailable_body.to_vec())
        .unwrap()
        .contains("wallet provider unavailable"));

    let malformed_dir = tempfile::tempdir().unwrap();
    let malformed_authority = passkey_authority(malformed_dir.path());
    let malformed_token = app_token_for_authority(
        malformed_dir.path(),
        WALLET_CAPSULE_ID,
        &malformed_authority,
    );
    let import_intent = json!({
        "recovery_key": {"opaque": "provider-owned-recovery-material"},
        "label": "Recovered",
    });
    let malformed_step_up = step_up_token_for_app_context(
        malformed_dir.path(),
        WALLET_CAPSULE_ID,
        &malformed_token,
        "wallet.recovery-key.import",
        &import_intent,
    );
    let malformed_state = wallet_test_state_with_shared_provider(
        malformed_dir.path(),
        Arc::new(MalformedRecoveryWalletProvider),
    )
    .await;
    let malformed = gateway_router(malformed_state)
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/wallet/wallet/accounts/import-recovery-key")
                .header("x-elastos-home-token", malformed_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "step_up_token": malformed_step_up,
                        "recovery_key": import_intent["recovery_key"],
                        "label": import_intent["label"],
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let malformed_body = axum::body::to_bytes(malformed.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8(malformed_body.to_vec())
        .unwrap()
        .contains("invalid Wallet provider v2 response"));
}
