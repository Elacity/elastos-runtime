use super::super::support::*;
use super::super::*;

fn access_payload(
    context: &VerifiedWalletInvocationContext,
    account_id: &str,
    address: &str,
    grant_expires_at: u64,
) -> Value {
    json!({
        "schema": "elastos.browser.account-access-request/v1",
        "permission": "eth_accounts",
        "principal_id": context.principal_id(),
        "session_id": context.session_id(),
        "launch_id": context.launch_id(),
        "proof_binding_id": context.proof_binding_id(),
        "origin": "https://dapp.example",
        "page_url": "https://dapp.example/connect",
        "account_id": account_id,
        "requested_chain_namespace": "eip155:20",
        "chain_namespaces": ["eip155:20", "eip155:8453"],
        "address": address,
        "grant_expires_at": grant_expires_at,
        "requires_wallet_approval": true
    })
}

fn managed_account(
    provider: &mut WalletProvider,
    context: &VerifiedWalletInvocationContext,
) -> (String, String) {
    match invoke_wallet_with_context(
        provider,
        context,
        WalletProviderOperationV2::CreateManagedAccount {
            chain_namespace: "eip155:20".into(),
            label: None,
            create_new: false,
        },
    ) {
        Response::Ok { data: Some(data) } => (
            data["account"]["account_id"].as_str().unwrap().to_string(),
            data["account"]["address"].as_str().unwrap().to_string(),
        ),
        other => panic!("expected managed account, got {other:?}"),
    }
}

#[test]
fn browser_account_access_is_disclosed_only_after_exact_managed_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:account-access", "browser");
    let (account_id, address) = managed_account(&mut provider, &context);
    let request_id = match invoke_wallet_with_context(
        &mut provider,
        &context,
        WalletProviderOperationV2::RequestApproval {
            account_id: account_id.clone(),
            chain_namespace: "eip155:20".into(),
            intent: "browser_account_access".into(),
            resource: format!("elastos://wallet/account/{account_id}/permission/eth_accounts"),
            reason: "Browser origin requests exact account access".into(),
            payload: access_payload(
                &context,
                &account_id,
                &address,
                now_ts().saturating_add(600),
            ),
            expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            assert_eq!(data["signature"], Value::Null);
            assert_eq!(data["approval_request"]["status"], "pending");
            data["approval_request"]["request_id"]
                .as_str()
                .unwrap()
                .to_string()
        }
        other => panic!("expected account-access approval, got {other:?}"),
    };

    match invoke_wallet_with_context(
        &mut provider,
        &context,
        WalletProviderOperationV2::ApproveAndSignManaged {
            request_id,
            reason: "Exact request reviewed in Wallet".into(),
        },
    ) {
        Response::Ok { data: Some(data) } => {
            let result = &data["approval_request"]["signed_result"];
            assert_eq!(result["schema"], "elastos.browser.account-access-result/v1");
            assert_eq!(result["permission"], "eth_accounts");
            assert_eq!(result["origin"], "https://dapp.example");
            assert_eq!(
                result["chain_namespaces"],
                json!(["eip155:20", "eip155:8453"])
            );
            assert_eq!(result["address"], address);
            assert!(result.get("signature").is_none());
            assert!(data["signature"]
                .as_str()
                .is_some_and(|value| value.starts_with("0x")));
        }
        other => panic!("expected completed account-access approval, got {other:?}"),
    }
}

#[test]
fn browser_account_access_rejects_substituted_authority_and_networks() {
    let dir = tempfile::tempdir().unwrap();
    let mut provider = init_provider(dir.path());
    let context = wallet_context("person:local:account-access-reject", "browser");
    let (account_id, address) = managed_account(&mut provider, &context);

    for (field, replacement) in [
        ("session_id", json!("session:substituted")),
        ("launch_id", json!("launch:substituted")),
        ("proof_binding_id", json!("proof:substituted")),
        ("origin", json!("https://attacker.example")),
        ("chain_namespaces", json!(["eip155:20", "eip155:1"])),
        ("grant_expires_at", json!(now_ts().saturating_sub(1))),
    ] {
        let mut payload = access_payload(
            &context,
            &account_id,
            &address,
            now_ts().saturating_add(600),
        );
        payload[field] = replacement;
        match invoke_wallet_with_context(
            &mut provider,
            &context,
            WalletProviderOperationV2::RequestApproval {
                account_id: account_id.clone(),
                chain_namespace: "eip155:20".into(),
                intent: "browser_account_access".into(),
                resource: format!("elastos://wallet/account/{account_id}/permission/eth_accounts"),
                reason: "Browser origin requests exact account access".into(),
                payload,
                expires_at: now_ts().saturating_add(APPROVAL_REQUEST_TTL_SECS),
            },
        ) {
            Response::Error { code, .. } => assert_eq!(code, "invalid_browser_account_access"),
            other => panic!("expected {field} substitution rejection, got {other:?}"),
        }
    }
}
