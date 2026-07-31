use super::support::*;
use super::*;

#[test]
fn link_account_persists_and_lists_active_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());

    let response = invoke_wallet(
        &mut provider,
        "person:local:alice",
        "wallet-metamask",
        WalletProviderOperationV2::LinkVerifiedAccount {
            proof_binding_id: "proof:eip155:20:0xabc".into(),
            chain_namespace: "eip155:20".into(),
            address: "0xabc".into(),
            proof_type: "siwe".into(),
            label: Some("ESC".into()),
        },
    );
    assert!(matches!(response, Response::Ok { .. }));

    let mut provider = init_provider(dir.path());
    match invoke_wallet(
        &mut provider,
        "person:local:alice",
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let accounts = data["accounts"].as_array().unwrap();
            assert_eq!(accounts.len(), 1);
            assert_eq!(accounts[0]["account_id"], "wallet:eip155:20:0xabc");
            assert_eq!(accounts[0]["connector_id"], "wallet-metamask");
            assert_eq!(accounts[0]["signing_available"], true);
        }
        other => panic!("expected accounts, got {other:?}"),
    }
}

#[test]
fn managed_accounts_report_unavailable_signing_when_key_cannot_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";

    let response = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Spending".into()),
            create_new: true,
        },
    );
    let account_id = match response {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["accounts"][0]["signing_available"], true);
            assert_eq!(
                data["accounts"][0]["signing_status"],
                "managed_key_available"
            );
        }
        other => panic!("expected accounts, got {other:?}"),
    }

    provider.store.managed_wallets[0].ciphertext = "00".to_string();
    provider.save().unwrap();
    let mut provider = init_provider(dir.path());
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["accounts"][0]["signing_available"], false);
            assert_eq!(
                data["accounts"][0]["signing_status"],
                "managed_key_unavailable"
            );
        }
        other => panic!("expected accounts, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::RequestApproval {
            account_id,
            chain_namespace: "eip155:20".into(),
            intent: "publish_envelope".into(),
            resource: "elastos://content/publish".into(),
            reason: "Publish document revision".into(),
            payload: json!({"cid": "bafy-broken-key"}),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "managed_key_unavailable");
            assert!(message.contains("recover or recreate"));
        }
        other => panic!("expected unavailable managed key rejection, got {other:?}"),
    }
}

#[test]
fn create_managed_account_replaces_unavailable_idempotent_account() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";

    let old_account_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:8453".into(),
            label: Some("Base".into()),
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    provider.store.managed_wallets[0].ciphertext = "00".to_string();
    provider.save().unwrap();

    let mut provider = init_provider(dir.path());
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:8453".into(),
            label: Some("Base replacement".into()),
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], true);
            assert_eq!(data["account"]["chain_namespace"], "eip155:8453");
            assert_ne!(data["account"]["account_id"], old_account_id);
        }
        other => panic!("expected replacement managed account, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let accounts = data["accounts"].as_array().unwrap();
            assert!(accounts.iter().any(|account| {
                account["account_id"] == old_account_id
                    && account["signing_status"] == "managed_key_unavailable"
            }));
            assert!(accounts.iter().any(|account| {
                account["label"] == "Base replacement"
                    && account["signing_status"] == "managed_key_available"
            }));
        }
        other => panic!("expected accounts, got {other:?}"),
    }
}

#[test]
fn external_wallet_link_derives_connector_and_rejects_caller_override() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());

    let context = wallet_context("person:local:alice", "wallet-metamask");
    let request = wallet_request(
        &context,
        WalletProviderOperationV2::LinkVerifiedAccount {
            proof_binding_id: "proof:eip155:20:0xabc".into(),
            chain_namespace: "eip155:20".into(),
            address: "0xabc".into(),
            proof_type: "siwe".into(),
            label: None,
        },
    );
    let mut request = serde_json::to_value(request).unwrap();
    request["operation"]["params"]["connector_id"] = json!("wallet-unisat");
    match decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "wallet_contract",
            "request": request,
            "_runtime_invocation": runtime_invocation_envelope(),
        }),
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_wallet_contract");
            assert!(message.contains("connector_id"));
        }
        other => panic!("expected connector rejection, got {other:?}"),
    }
}

#[test]
fn create_managed_account_persists_encrypted_key_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";

    let first = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Passkey approval".into()),
            create_new: false,
        },
    );
    let account_id = match first {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], true);
            let account = &data["account"];
            assert_eq!(account["principal_id"], principal_id);
            assert_eq!(account["chain_namespace"], "eip155:20");
            assert_eq!(account["proof_type"], MANAGED_EVM_PROOF_TYPE);
            assert!(account["address"].as_str().unwrap().starts_with("0x"));
            assert!(account.get("ciphertext").is_none());
            account["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    assert_eq!(provider.store.managed_wallets.len(), 1);
    assert_eq!(provider.store.managed_wallets[0].account_id, account_id);
    assert!(!provider.store.managed_wallets[0].ciphertext.is_empty());
    let secret = provider.store.managed_wallets[0].clone();
    assert!(provider.decrypt_managed_key(&secret).is_ok());
    let mut tampered_principal = secret.clone();
    tampered_principal.principal_id = "person:local:bob".to_string();
    assert!(provider.decrypt_managed_key(&tampered_principal).is_err());
    let mut tampered_chain = secret.clone();
    tampered_chain.chain_namespace = "eip155:8453".to_string();
    assert!(provider.decrypt_managed_key(&tampered_chain).is_err());

    let state_path = dir
        .path()
        .join("ElastOS")
        .join("SystemServices")
        .join("Wallet")
        .join("wallet-state.json");
    let state = fs::read_to_string(state_path).unwrap();
    assert!(state.contains("managed_secret"));
    assert!(!state.contains("private_key"));

    let mut provider = init_provider(dir.path());
    let second = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    );
    match second {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], false);
            assert_eq!(data["account"]["account_id"], account_id);
        }
        other => panic!("expected existing managed account, got {other:?}"),
    }
}

#[test]
fn create_managed_account_can_create_new_passkey_account_on_same_network() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";

    let first = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:8453".into(),
            label: Some("Spending".into()),
            create_new: false,
        },
    );
    let first_address = match first {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], true);
            data["account"]["address"].as_str().unwrap().to_string()
        }
        other => panic!("expected first managed account, got {other:?}"),
    };

    let second = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:8453".into(),
            label: Some("Agent Budget".into()),
            create_new: true,
        },
    );
    match second {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], true);
            assert_eq!(data["account"]["label"], "Agent Budget");
            assert_ne!(data["account"]["address"], first_address);
        }
        other => panic!("expected additional managed account, got {other:?}"),
    }
    assert_eq!(provider.store.accounts.len(), 2);
    assert_eq!(provider.store.managed_wallets.len(), 2);
}

#[test]
fn create_managed_account_reuses_principal_key_across_evm_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";

    let first = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    );
    let first_address = match first {
        Response::Ok { data: Some(data) } => data["account"]["address"]
            .as_str()
            .expect("managed account address")
            .to_string(),
        other => panic!("expected first managed account, got {other:?}"),
    };

    let second = invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:8453".into(),
            label: None,
            create_new: false,
        },
    );
    match second {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], true);
            assert_eq!(data["account"]["chain_namespace"], "eip155:8453");
            assert_eq!(data["account"]["address"], first_address);
        }
        other => panic!("expected second managed account, got {other:?}"),
    }
    assert_eq!(provider.store.managed_wallets.len(), 2);
}

#[test]
fn create_managed_btc_account_uses_p2wpkh_and_separate_key_scope() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";

    let evm_address = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => data["account"]["address"]
            .as_str()
            .expect("EVM managed address")
            .to_string(),
        other => panic!("expected EVM managed account, got {other:?}"),
    };

    let (btc_account_id, btc_address) = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            label: Some("Bitcoin".into()),
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], true);
            let account = &data["account"];
            assert_eq!(account["chain_namespace"], BITCOIN_MAINNET_CHAIN_NAMESPACE);
            assert_eq!(account["proof_type"], MANAGED_BTC_P2WPKH_PROOF_TYPE);
            assert_eq!(account["label"], "Bitcoin");
            let address = account["address"].as_str().unwrap().to_string();
            assert!(address.starts_with("bc1q"));
            assert_ne!(address, evm_address);
            (account["account_id"].as_str().unwrap().to_string(), address)
        }
        other => panic!("expected BTC managed account, got {other:?}"),
    };
    assert_eq!(provider.store.managed_wallets.len(), 2);

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: BITCOIN_MAINNET_CHAIN_NAMESPACE.into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["created"], false);
            assert_eq!(data["account"]["account_id"], btc_account_id);
            assert_eq!(data["account"]["address"], btc_address);
        }
        other => panic!("expected existing BTC managed account, got {other:?}"),
    }
}

#[test]
fn default_account_is_principal_scoped_and_drives_explicit_signature_requests() {
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
                label: Some("MetaMask".into()),
            },
        ),
        Response::Ok { .. }
    ));
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::SetDefaultAccount {
            chain_namespace: "eip155:20".into(),
            intent: "publish_envelope".into(),
            account_id: account_id.into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["default_account"]["account_id"], account_id);
            assert_eq!(data["default_account"]["intent"], "publish_envelope");
        }
        other => panic!("expected default account, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::SetDefaultAccount {
            chain_namespace: "eip155:20".into(),
            intent: "browser_connect".into(),
            account_id: account_id.into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["default_account"]["account_id"], account_id);
            assert_eq!(data["default_account"]["intent"], "browser_connect");
        }
        other => panic!("expected browser default account, got {other:?}"),
    }

    let mut provider = init_provider(dir.path());
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["accounts"].as_array().unwrap().len(), 1);
            assert_eq!(data["default_accounts"].as_array().unwrap().len(), 2);
            assert_eq!(data["default_accounts"][0]["account_id"], account_id);
        }
        other => panic!("expected accounts with defaults, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "documents",
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.to_string(),
            chain_namespace: "eip155:20".into(),
            intent: "publish_envelope".into(),
            resource: "elastos://content/publish".into(),
            reason: "Publish document revision".into(),
            payload: json!({"cid": "bafy-default"}),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let approval = &data["approval_request"];
            assert_eq!(approval["account_id"], account_id);
            assert_eq!(approval["proof_type"], "siwe");
            assert_eq!(approval["connector_id"], "wallet-metamask");
        }
        other => panic!("expected default-backed approval request, got {other:?}"),
    }
}

#[test]
fn evm_default_account_is_latest_wins_across_eip155_chains() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let esc_address = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let ethereum_address = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let esc_account_id = "wallet:eip155:20:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let ethereum_account_id = "wallet:eip155:1:0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: "proof:eip155:20:0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .into(),
                chain_namespace: "eip155:20".into(),
                address: esc_address.into(),
                proof_type: "siwe".into(),
                label: Some("ESC".into()),
            },
        ),
        Response::Ok { .. }
    ));
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet-metamask",
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id: "proof:eip155:1:0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .into(),
                chain_namespace: "eip155:1".into(),
                address: ethereum_address.into(),
                proof_type: "siwe".into(),
                label: Some("Ethereum".into()),
            },
        ),
        Response::Ok { .. }
    ));

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet",
            WalletProviderOperationV2::SetDefaultAccount {
                chain_namespace: "eip155:1".into(),
                intent: "transaction_intent".into(),
                account_id: ethereum_account_id.into(),
            },
        ),
        Response::Ok { .. }
    ));
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet",
            WalletProviderOperationV2::SetDefaultAccount {
                chain_namespace: "eip155:20".into(),
                intent: "transaction_intent".into(),
                account_id: esc_account_id.into(),
            },
        ),
        Response::Ok { .. }
    ));

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let defaults = data["default_accounts"].as_array().unwrap();
            assert_eq!(defaults.len(), 1);
            assert_eq!(defaults[0]["account_id"], esc_account_id);
            assert_eq!(defaults[0]["chain_namespace"], "eip155:20");
        }
        other => panic!("expected accounts with one EVM default, got {other:?}"),
    }

    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet",
            WalletProviderOperationV2::SetDefaultAccount {
                chain_namespace: "eip155:1".into(),
                intent: "transaction_intent".into(),
                account_id: ethereum_account_id.into(),
            },
        ),
        Response::Ok { .. }
    ));

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::DefaultAccount {
            chain_namespace: "eip155:20".into(),
            intent: "publish_envelope".into(),
        },
    ) {
        Response::Error { code, .. } => assert_eq!(code, "not_found"),
        other => panic!("expected missing publish default, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::DefaultAccount {
            chain_namespace: "eip155:20".into(),
            intent: "transaction_intent".into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["default_account"]["account_id"], ethereum_account_id);
        }
        other => panic!("expected latest EVM default, got {other:?}"),
    }
    match invoke_wallet(
        &mut provider,
        principal_id,
        "browser",
        WalletProviderOperationV2::RequestApproval {
            account_id: ethereum_account_id.to_string(),
            chain_namespace: "eip155:20".into(),
            intent: "transaction_intent".into(),
            resource: "elastos://wallet/eip155:20/sign/transaction_intent".into(),
            reason: "Browser transaction".into(),
            payload: json!({
                "schema": "elastos.chain.unsigned_transaction_intent/v1",
                "transaction_type": "eip155_legacy",
                "chain_id": 20,
                "from": ethereum_address,
                "to": "0x0000000000000000000000000000000000000001",
                "value": "0x0",
                "data": "0x",
                "gas_limit": "0x5208",
                "gas_price": "0x1",
                "nonce": "0x0",
                "requires_wallet_approval": true,
                "wallet_intent": "transaction_intent"
            }),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["approval_request"]["account_id"], ethereum_account_id);
            assert_eq!(data["approval_request"]["chain_namespace"], "eip155:20");
        }
        other => panic!("expected latest EVM default approval request, got {other:?}"),
    }
}

#[test]
fn default_account_rejects_cross_chain_or_missing_accounts() {
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

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::SetDefaultAccount {
            chain_namespace: "eip155:8453".into(),
            intent: "publish_envelope".into(),
            account_id: account_id.into(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_request");
            assert!(message.contains("chain"));
        }
        other => panic!("expected cross-chain rejection, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::DefaultAccount {
            chain_namespace: "eip155:20".into(),
            intent: "credential".into(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "not_found");
            assert!(message.contains("default"));
        }
        other => panic!("expected missing-default rejection, got {other:?}"),
    }
}

#[test]
fn create_managed_account_requires_explicit_chain_namespace() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:alice", "wallet");
    let request = wallet_request(
        &context,
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    );
    let mut request = serde_json::to_value(request).unwrap();
    request["operation"]["params"]
        .as_object_mut()
        .unwrap()
        .remove("chain_namespace");
    let response = decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "wallet_contract",
            "request": request,
            "_runtime_invocation": runtime_invocation_envelope(),
        }),
    );
    assert!(matches!(
        response,
        Response::Error { ref code, .. } if code == "invalid_wallet_contract"
    ));
}

#[test]
fn managed_namespace_errors_are_current() {
    let evm_error = validate_evm_chain_namespace(BITCOIN_MAINNET_CHAIN_NAMESPACE).unwrap_err();
    assert_eq!(
        evm_error,
        "managed EVM wallets require an eip155 chain namespace"
    );

    let managed_error = managed_proof_type("nostr:alice").unwrap_err();
    assert_eq!(
        managed_error,
        "managed wallets support EVM chains and Bitcoin mainnet P2WPKH"
    );
    assert!(!managed_error.contains("currently require"));
}

#[test]
fn revoke_account_hides_account_by_default() {
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
    assert!(matches!(
        invoke_wallet(
            &mut provider,
            principal_id,
            "wallet",
            WalletProviderOperationV2::RevokeAccount {
                account_id: account_id.into(),
            },
        ),
        Response::Ok { .. }
    ));

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert!(data["accounts"].as_array().unwrap().is_empty());
        }
        other => panic!("expected accounts, got {other:?}"),
    }
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: true,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["accounts"].as_array().unwrap().len(), 1);
        }
        other => panic!("expected revoked account, got {other:?}"),
    }
}

#[test]
fn rename_account_updates_active_label_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:8453".into(),
            label: Some("Spending".into()),
            create_new: true,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::RenameAccount {
            account_id: account_id.clone(),
            label: "  Savings  ".into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["account"]["label"], "Savings");
        }
        other => panic!("expected renamed account, got {other:?}"),
    }
    let context = wallet_context(principal_id, "wallet");
    let request = wallet_request(
        &context,
        WalletProviderOperationV2::RenameAccount {
            account_id,
            label: "placeholder".into(),
        },
    );
    let mut request = serde_json::to_value(request).unwrap();
    request["operation"]["params"]["label"] = json!("");
    match decode_and_handle_outer(
        &mut provider,
        json!({
            "op": "wallet_contract",
            "request": request,
            "_runtime_invocation": runtime_invocation_envelope(),
        }),
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "invalid_wallet_contract");
            assert!(message.contains("label"));
        }
        other => panic!("expected blank label rejection, got {other:?}"),
    }
}

#[test]
fn export_managed_recovery_key_is_available_for_active_managed_account_only() {
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
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ExportManagedRecoveryKey {
            account_id: account_id.clone(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["schema"], "elastos.wallet.recovery-key/v1");
            assert_eq!(data["account_id"], account_id);
            assert_eq!(data["secret_type"], "secp256k1_private_key_hex");
            assert_eq!(data["private_key_hex"].as_str().unwrap().len(), 64);
        }
        other => panic!("expected recovery key export, got {other:?}"),
    }

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
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ExportManagedRecoveryKey {
            account_id: "wallet:eip155:20:0xabc".into(),
        },
    ) {
        Response::Error { code, message } => {
            assert_eq!(code, "external_wallet_required");
            assert!(message.contains("passkey-managed"));
        }
        other => panic!("expected external-wallet rejection, got {other:?}"),
    }
}

#[test]
fn import_managed_recovery_key_restores_exported_wallet() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let principal_id = "person:local:alice";
    let account_id = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: Some("Spending".into()),
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            data["account"]["account_id"].as_str().unwrap().to_string()
        }
        other => panic!("expected managed account, got {other:?}"),
    };
    let recovery_key = match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ExportManagedRecoveryKey {
            account_id: account_id.clone(),
        },
    ) {
        Response::Ok { data: Some(data) } => data,
        other => panic!("expected recovery key export, got {other:?}"),
    };

    provider.store.managed_wallets[0].ciphertext = "00".to_string();
    provider.save().unwrap();
    let mut provider = init_provider(dir.path());
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(
                data["accounts"][0]["signing_status"],
                "managed_key_unavailable"
            );
        }
        other => panic!("expected accounts, got {other:?}"),
    }

    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ImportManagedRecoveryKey {
            recovery_key,
            label: Some("Recovered".into()),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["imported"], true);
            assert_eq!(data["account"]["account_id"], account_id);
            assert_eq!(data["account"]["label"], "Recovered");
        }
        other => panic!("expected recovery key import, got {other:?}"),
    }
    match invoke_wallet(
        &mut provider,
        principal_id,
        "wallet",
        WalletProviderOperationV2::ListAccounts {
            include_revoked: false,
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(
                data["accounts"][0]["signing_status"],
                "managed_key_available"
            );
        }
        other => panic!("expected accounts, got {other:?}"),
    }
}
