use super::support::*;
use super::*;
use std::fs;

fn wallet_outer(request: &WalletProviderRequestV2, envelope: Value) -> Value {
    json!({
        "op": "wallet_contract",
        "request": request,
        "_runtime_invocation": envelope,
    })
}

fn write_pre_v2_pending_approval_store(dir: &Path) -> PathBuf {
    let mut provider = init_provider(dir);
    let principal_id = "person:local:alice";
    let account_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "documents",
            WalletProviderOperationV2::RequestApproval {
                account_id,
                chain_namespace: "eip155:20".into(),
                intent: "publish_envelope".into(),
                resource: "elastos://content/publish".into(),
                reason: "Legacy pending request".into(),
                payload: json!({ "cid": "bafy-legacy" }),
                expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
            },
        ),
        Response::Ok { .. }
    ));

    let store_path = provider.store_path.clone().unwrap();
    let mut stored: Value = serde_json::from_slice(&fs::read(&store_path).unwrap()).unwrap();
    let approval = stored["approval_requests"][0].as_object_mut().unwrap();
    approval.remove("session_id");
    approval.remove("launch_id");
    approval.remove("wallet_request_sha256");
    approval.remove("authority_binding");
    approval.remove("validated_chain_outcome");
    let requested_by_actor = approval.remove("requested_by_actor").unwrap();
    approval.insert("capsule_id".to_string(), requested_by_actor);
    fs::write(&store_path, serde_json::to_vec_pretty(&stored).unwrap()).unwrap();
    store_path
}

#[test]
fn request_approval_uses_the_runtime_request_identity_and_replays_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    let context = wallet_context(principal_id, "system");
    let operation = WalletProviderOperationV2::RequestApproval {
        account_id,
        chain_namespace: "eip155:20".into(),
        intent: "transaction_intent".into(),
        resource: "elastos://chain/esc-mainnet/broadcast_transaction".into(),
        reason: "Stable approval".into(),
        payload: transaction_intent_payload(&provider.store.accounts[0].address),
        expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
    };
    let request = wallet_request(&context, operation.clone());

    for _ in 0..2 {
        match invoke_wallet_request(&mut provider, &request) {
            Response::Ok { data: Some(data) } => {
                assert_eq!(data["approval_request"]["request_id"], request.request_id);
            }
            other => panic!("expected idempotent approval, got {other:?}"),
        }
    }
    assert_eq!(provider.store.approval_requests.len(), 1);

    let mut substituted = match operation {
        WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace,
            intent,
            resource,
            reason: _,
            payload,
            expires_at,
        } => WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace,
            intent,
            resource,
            reason: "Substituted approval".into(),
            payload,
            expires_at,
        },
        _ => unreachable!(),
    };
    let now = now_ts();
    let substituted_request = WalletProviderRequestV2::new(
        &context,
        request.request_id.clone(),
        now,
        now.saturating_add(120),
        substituted.clone(),
    )
    .unwrap();
    assert!(matches!(
        invoke_wallet_request(&mut provider, &substituted_request),
        Response::Error { ref code, .. } if code == "approval_identity_conflict"
    ));

    if let WalletProviderOperationV2::RequestApproval { reason, .. } = &mut substituted {
        *reason = "Stable approval".into();
    }
    let foreign = wallet_context_in_session(principal_id, "system", "session:other");
    let foreign_request = WalletProviderRequestV2::new(
        &foreign,
        request.request_id.clone(),
        now,
        now.saturating_add(120),
        substituted,
    )
    .unwrap();
    assert!(matches!(
        invoke_wallet_request(&mut provider, &foreign_request),
        Response::Error { ref code, .. } if code == "approval_identity_conflict"
    ));
}

#[test]
fn production_outer_decoder_rejects_retired_and_generic_wallet_operations() {
    for operation in [
        "accounts",
        "create_managed_account",
        "link_account",
        "request_signature",
        "approval_requests",
        "approve_approval",
        "complete_approval",
        "sign_approved",
        "record_transaction_hash",
    ] {
        let error = serde_json::from_value::<Request>(json!({ "op": operation }))
            .expect_err("retired operation must not reach production dispatch")
            .to_string();
        assert!(error.contains("unknown variant"), "{operation}: {error}");
    }
}

#[test]
fn wallet_contract_requires_the_exact_runtime_local_json_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:alice", "wallet");
    let request = wallet_request(
        &context,
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    );

    let missing = decode_and_handle_outer(
        &mut provider,
        json!({ "op": "wallet_contract", "request": request }),
    );
    assert!(matches!(missing, Response::Error { ref code, .. } if code == "invalid_request"));

    for (field, value) in [
        ("source", json!("capsule")),
        ("target", json!("chain")),
        ("op", json!("status")),
        ("capability", json!("provider:runtime->wallet:status")),
        ("transport", json!("carrier-provider-plane")),
        ("transfer", json!("bytes")),
        ("carrier", json!({ "route": "connect_ticket" })),
        ("range", json!({ "start": 0, "end": 1 })),
        ("progress", json!({ "request_id": "forged" })),
    ] {
        let mut envelope = runtime_invocation_envelope();
        envelope[field] = value;
        let response = decode_and_handle_outer(&mut provider, wallet_outer(&request, envelope));
        assert!(
            matches!(response, Response::Error { ref code, .. } if code == "invalid_runtime_invocation"),
            "field {field} unexpectedly reached Wallet: {response:?}"
        );
    }

    let mut unknown = runtime_invocation_envelope();
    unknown["forged"] = json!(true);
    let response = decode_and_handle_outer(&mut provider, wallet_outer(&request, unknown));
    assert!(matches!(response, Response::Error { ref code, .. } if code == "invalid_request"));

    let mut forged_request = serde_json::to_value(&request).unwrap();
    forged_request["_runtime_invocation"] = runtime_invocation_envelope();
    let response = decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "wallet_contract",
            "request": forged_request,
            "_runtime_invocation": runtime_invocation_envelope(),
        }),
    );
    assert!(
        matches!(response, Response::Error { ref code, .. } if code == "invalid_wallet_contract")
    );

    let mut forged_outer = wallet_outer(&request, runtime_invocation_envelope());
    forged_outer["_runtime_transfer"] = json!({ "schema": "forged" });
    let response = decode_and_handle_outer(&mut provider, forged_outer);
    assert!(matches!(response, Response::Error { ref code, .. } if code == "invalid_request"));
}

#[test]
fn wallet_contract_rejects_missing_and_mixed_protocol_versions() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:alice", "wallet");
    let request = wallet_request(
        &context,
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("must not dispatch".into()),
            create_new: true,
        },
    );
    for version in [Some("1.0"), Some("2.0"), Some("2.1"), None] {
        let mut value = serde_json::to_value(&request).unwrap();
        match version {
            Some(version) => value["protocol_version"] = json!(version),
            None => {
                value.as_object_mut().unwrap().remove("protocol_version");
            }
        }
        let response = decode_and_handle_outer(
            &mut provider,
            json!({
                "op": "wallet_contract",
                "request": value,
                "_runtime_invocation": runtime_invocation_envelope(),
            }),
        );
        assert!(
            matches!(response, Response::Error { ref code, .. } if code == "invalid_wallet_contract"),
            "mixed version unexpectedly reached Wallet: {response:?}"
        );
        assert!(provider.store.accounts.is_empty());
        assert!(provider.store.managed_wallets.is_empty());
        assert!(provider.store.consumed_lifecycles.is_empty());
    }
}

#[test]
fn effectful_lifecycle_replay_is_persistent_while_reads_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:alice", "wallet");
    let create = wallet_request(
        &context,
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Replay proof".into()),
            create_new: true,
        },
    );
    assert!(matches!(
        invoke_wallet_request(&mut provider, &create),
        Response::Ok { .. }
    ));
    assert!(matches!(
        invoke_wallet_request(&mut provider, &create),
        Response::Error { ref code, .. } if code == "lifecycle_replay"
    ));
    assert_eq!(provider.store.accounts.len(), 1);
    let marker = provider
        .store
        .consumed_lifecycles
        .iter()
        .find(|record| record.lifecycle_id == create.lifecycle_id)
        .unwrap();
    assert_eq!(marker.request_expires_at, create.expires_at);

    let mut provider = init_provider(dir.path());
    assert!(matches!(
        invoke_wallet_request(&mut provider, &create),
        Response::Error { ref code, .. } if code == "lifecycle_replay"
    ));
    assert_eq!(provider.store.accounts.len(), 1);

    let read = wallet_request(
        &context,
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    );
    assert!(matches!(
        invoke_wallet_request(&mut provider, &read),
        Response::Ok { .. }
    ));
    assert!(matches!(
        invoke_wallet_request(&mut provider, &read),
        Response::Ok { .. }
    ));
}

#[test]
fn expired_lifecycle_markers_are_pruned_without_evicting_active_markers() {
    let now = now_ts();
    let mut store = WalletStore::default();
    for index in 0..(MAX_EFFECTFUL_LIFECYCLE_HISTORY + 40) {
        store.consumed_lifecycles.push(ConsumedWalletLifecycle {
            lifecycle_id: format!("lifecycle:{index}"),
            request_sha256: format!("sha:{index}"),
            request_expires_at: now.saturating_add(60),
            consumed_at: now.saturating_sub(index as u64),
        });
    }
    store.consumed_lifecycles.push(ConsumedWalletLifecycle {
        lifecycle_id: "expired".into(),
        request_sha256: "expired".into(),
        request_expires_at: now,
        consumed_at: now.saturating_sub(1),
    });
    let store = prune_store(store, now);
    assert_eq!(
        store.consumed_lifecycles.len(),
        MAX_EFFECTFUL_LIFECYCLE_HISTORY + 40
    );
    assert!(store
        .consumed_lifecycles
        .iter()
        .all(|record| record.lifecycle_id != "expired"));
    assert!(store
        .consumed_lifecycles
        .iter()
        .any(|record| record.lifecycle_id == "lifecycle:0"));
}

#[test]
fn lifecycle_capacity_rejects_before_mutation_and_preserves_the_oldest_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:alice", "wallet");
    let oldest = wallet_request(
        &context,
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Oldest".into()),
            create_new: true,
        },
    );
    assert!(matches!(
        invoke_wallet_request(&mut provider, &oldest),
        Response::Ok { .. }
    ));

    let now = now_ts();
    while provider.store.consumed_lifecycles.len() < MAX_EFFECTFUL_LIFECYCLE_HISTORY {
        let index = provider.store.consumed_lifecycles.len();
        provider
            .store
            .consumed_lifecycles
            .push(ConsumedWalletLifecycle {
                lifecycle_id: format!("lifecycle:capacity:{index}"),
                request_sha256: format!("sha:capacity:{index}"),
                request_expires_at: oldest.expires_at,
                consumed_at: now,
            });
    }
    provider.save().unwrap();
    let account_count = provider.store.accounts.len();
    let saturated = wallet_request(
        &context,
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Must not be created".into()),
            create_new: true,
        },
    );
    assert!(matches!(
        invoke_wallet_request(&mut provider, &saturated),
        Response::Error { ref code, .. } if code == "lifecycle_capacity"
    ));
    assert_eq!(provider.store.accounts.len(), account_count);
    assert_eq!(
        provider.store.consumed_lifecycles.len(),
        MAX_EFFECTFUL_LIFECYCLE_HISTORY
    );
    assert!(matches!(
        invoke_wallet_request(&mut provider, &oldest),
        Response::Error { ref code, .. } if code == "lifecycle_replay"
    ));

    let mut provider = init_provider(dir.path());
    assert_eq!(
        provider.store.consumed_lifecycles.len(),
        MAX_EFFECTFUL_LIFECYCLE_HISTORY
    );
    assert!(matches!(
        invoke_wallet_request(&mut provider, &oldest),
        Response::Error { ref code, .. } if code == "lifecycle_replay"
    ));
    assert_eq!(provider.store.accounts.len(), account_count);
}

#[test]
fn managed_atomic_signing_rolls_back_and_allows_safe_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace: "eip155:20".into(),
            intent: "publish_envelope".into(),
            resource: "elastos://content/publish".into(),
            reason: "Publish document revision".into(),
            payload: json!({ "cid": "bafy-atomic" }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected approval, got {other:?}"),
    };

    let context = wallet_context(principal_id, "documents");
    let wrong_session = wallet_context_in_session(principal_id, "documents", "session:other");
    assert!(matches!(
        invoke_wallet_with_context(
            &mut provider,
            &wrong_session,
            WalletProviderOperationV2::ApproveAndSignManaged {
                request_id: request_id.clone(),
                reason: "Approved".into(),
            },
        ),
        Response::Error { ref code, .. } if code == "invalid_request"
    ));
    assert_eq!(
        provider.store.approval_requests[0].status,
        ApprovalStatus::Pending
    );
    let approve = wallet_request(
        &context,
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "Approved".into(),
        },
    );
    let secret = provider.store.managed_wallets[0].clone();
    provider.store.managed_wallets[0].ciphertext = "00".into();
    assert!(matches!(
        invoke_wallet_request(&mut provider, &approve),
        Response::Error { ref code, .. } if code == "storage_error"
    ));
    assert_eq!(
        provider.store.approval_requests[0].status,
        ApprovalStatus::Pending
    );
    assert!(!provider
        .store
        .consumed_lifecycles
        .iter()
        .any(|record| record.lifecycle_id == approve.lifecycle_id));

    provider.store.managed_wallets[0] = secret;
    assert!(matches!(
        invoke_wallet_request(&mut provider, &approve),
        Response::Ok { .. }
    ));
    assert_eq!(
        provider.store.approval_requests[0].status,
        ApprovalStatus::Completed
    );
}

#[test]
fn connector_handoff_requires_the_stored_actor_and_runtime_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = "wallet:eip155:20:0xabc";
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: "proof:eip155:20:0xabc".into(),
                chain_namespace: "eip155:20".into(),
                address: "0xabc".into(),
                proof_type: "siwe".into(),
                label: None,
            },
        ),
        Response::Ok { .. }
    ));
    let request_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.into(),
            chain_namespace: "eip155:20".into(),
            intent: "credential".into(),
            resource: "elastos://wallet/eip155:20/sign/credential".into(),
            reason: "Issue credential".into(),
            payload: json!({ "credential": "test" }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => data["approval_request"]["request_id"]
            .as_str()
            .unwrap()
            .to_string(),
        other => panic!("expected connector approval, got {other:?}"),
    };

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-unisat",
            WalletProviderOperationV2::ApproveConnectorHandoff {
                request_id: request_id.clone(),
                reason: "Wrong connector".into(),
            },
        ),
        Response::Error { ref code, .. } if code == "invalid_request"
    ));
    let wrong_session =
        wallet_context_in_session(principal_id, "wallet-metamask", "session:different");
    assert!(matches!(
        invoke_wallet_with_context(
            &mut provider,
            &wrong_session,
            WalletProviderOperationV2::ApproveConnectorHandoff {
                request_id: request_id.clone(),
                reason: "Wrong session".into(),
            },
        ),
        Response::Error { ref code, .. } if code == "invalid_request"
    ));
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::ApproveConnectorHandoff {
                request_id,
                reason: "Approved".into(),
            },
        ),
        Response::Ok { .. }
    ));
}

#[test]
fn pre_v2_migration_rejects_only_unresolved_history_and_writes_the_canonical_actor() {
    let dir = tempfile::tempdir().unwrap();
    let principal_id = "person:local:alice";
    let store_path = write_pre_v2_pending_approval_store(dir.path());
    let mut stored: Value = serde_json::from_slice(&fs::read(&store_path).unwrap()).unwrap();
    let mut completed = stored["approval_requests"][0].clone();
    completed["request_id"] = json!("wallet-approval:legacy-completed");
    completed["status"] = json!("completed");
    completed["completed_at"] = json!(now_ts());
    stored["approval_requests"]
        .as_array_mut()
        .unwrap()
        .push(completed);
    fs::write(&store_path, serde_json::to_vec_pretty(&stored).unwrap()).unwrap();

    let mut provider = WalletProvider::new();
    let init = decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "init",
            "config": { "base_path": dir.path().display().to_string() }
        }),
    );
    assert!(matches!(
        init,
        Response::Ok { data: Some(ref data) }
            if data["pre_v2_approvals_rejected"] == 1
    ));
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListApprovals {
            include_resolved: true,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let approvals = data["approval_requests"].as_array().unwrap();
            assert_eq!(approvals.len(), 2);
            let rejected = approvals
                .iter()
                .find(|approval| approval["status"] == "rejected")
                .unwrap();
            assert!(rejected["rejection_reason"]
                .as_str()
                .unwrap()
                .contains("pre-v2 approval preserved as history"));
            assert!(approvals.iter().any(|approval| {
                approval["request_id"] == "wallet-approval:legacy-completed"
                    && approval["status"] == "completed"
            }));
        }
        other => panic!("expected preserved legacy history, got {other:?}"),
    }
    let stored: Value = serde_json::from_slice(&fs::read(&store_path).unwrap()).unwrap();
    for approval in stored["approval_requests"].as_array().unwrap() {
        assert!(approval.get("requested_by_actor").is_some());
        assert!(approval.get("capsule_id").is_none());
    }

    let mut provider = WalletProvider::new();
    let init = decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "init",
            "config": { "base_path": dir.path().display().to_string() }
        }),
    );
    assert!(matches!(
        init,
        Response::Ok { data: Some(ref data) }
            if data["pre_v2_approvals_rejected"] == 0
    ));
    assert_eq!(provider.store.approval_requests.len(), 2);
    assert_eq!(
        provider
            .store
            .approval_requests
            .iter()
            .filter(|approval| approval.status == ApprovalStatus::Rejected)
            .count(),
        1
    );
    assert_eq!(
        provider
            .store
            .approval_requests
            .iter()
            .filter(|approval| approval.status == ApprovalStatus::Completed)
            .count(),
        1
    );
}

#[test]
fn failed_pre_v2_migration_save_leaves_provider_uninitialized() {
    let dir = tempfile::tempdir().unwrap();
    let store_path = write_pre_v2_pending_approval_store(dir.path());
    fs::create_dir(store_path.with_extension("json.tmp")).unwrap();

    let mut provider = WalletProvider::new();
    let init = decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "init",
            "config": { "base_path": dir.path().display().to_string() }
        }),
    );
    assert!(matches!(
        init,
        Response::Error { ref code, .. } if code == "storage_error"
    ));
    assert!(provider.store_path.is_none());
    assert!(provider.storage_key.is_none());
    assert!(provider.store.approval_requests.is_empty());
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            "person:local:alice",
            "wallet",
            WalletProviderOperationV2::ListAccounts {
                include_revoked: false,
            },
        ),
        Response::Error { ref code, .. } if code == "not_initialized"
    ));
}

#[test]
fn manifest_is_consistently_versioned_and_status_only() {
    let manifest: Value = serde_json::from_str(include_str!("../../capsule.json")).unwrap();
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.2.0");
    assert_eq!(manifest["version"], "0.2.0");
    assert_eq!(manifest["provides"], "elastos://wallet/meta/status");
    assert_eq!(manifest["interfaces"][0]["version"], "0.6.0");
    assert_eq!(
        manifest["interfaces"][0]["methods"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(manifest["interfaces"][0]["methods"][0]["id"], "status");
    assert_eq!(
        manifest["authority"]["capabilities"][0]["operations"],
        json!(["status"])
    );
    let serialized = serde_json::to_string(&manifest).unwrap();
    for retired in [
        "approve_approval",
        "complete_approval",
        "sign_approved",
        "record_transaction_hash",
        "wallet_contract",
    ] {
        assert!(!serialized.contains(retired), "manifest leaked {retired}");
    }
}
