//! Browser wallet bridge payload and account selection helpers.

use super::*;
use std::collections::HashSet;

const BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES: &[&str] = &["eip155:20", "eip155:8453"];

pub(in crate::api::gateway) fn is_browser_wallet_intent(intent: Option<&str>) -> bool {
    matches!(
        intent,
        Some("browser_personal_sign")
            | Some("browser_typed_data_sign")
            | Some("transaction_intent")
    )
}

pub(in crate::api::gateway) fn browser_chain_namespace_network(
    chain_namespace: &str,
) -> Option<&'static str> {
    match chain_namespace {
        "eip155:20" => Some("esc-mainnet"),
        "eip155:8453" => Some("base-mainnet"),
        _ => None,
    }
}

fn browser_default_evm_account(
    summary: &SystemWalletAccountsSummary,
) -> Option<&SystemWalletDefaultSummary> {
    let has_linked_evm_account = |default: &SystemWalletDefaultSummary| {
        summary.accounts.iter().any(|account| {
            account.account_id == default.account_id
                && browser_wallet_account_is_signable_evm(account)
        })
    };
    ["browser_connect", "transaction_intent"]
        .iter()
        .find_map(|intent| {
            summary
                .default_accounts
                .iter()
                .filter(|account| account.intent == *intent && has_linked_evm_account(account))
                .max_by_key(|account| account.set_at)
        })
}

fn browser_default_account_id(summary: &SystemWalletAccountsSummary) -> Option<String> {
    browser_default_evm_account(summary)
        .map(|default| default.account_id.clone())
        .or_else(|| {
            summary
                .accounts
                .iter()
                .find(|account| browser_wallet_account_is_signable_evm(account))
                .map(|account| account.account_id.clone())
        })
}

fn browser_default_chain_namespace(summary: &SystemWalletAccountsSummary) -> Option<String> {
    if let Some(default) = browser_default_evm_account(summary) {
        return if browser_chain_namespace_network(&default.chain_namespace).is_some() {
            Some(default.chain_namespace.clone())
        } else {
            Some("eip155:20".to_string())
        };
    }
    summary
        .accounts
        .iter()
        .find(|account| {
            browser_wallet_account_is_signable_evm(account)
                && browser_chain_namespace_network(&account.chain_namespace).is_some()
        })
        .map(|account| account.chain_namespace.clone())
}

fn browser_projected_evm_accounts(
    summary: &SystemWalletAccountsSummary,
) -> Vec<SystemWalletAccountSummary> {
    let default_account_id = browser_default_account_id(summary);
    let mut accounts = summary
        .accounts
        .iter()
        .filter(|account| browser_wallet_account_is_signable_evm(account))
        .cloned()
        .collect::<Vec<_>>();
    accounts.sort_by_key(|account| {
        if Some(account.account_id.as_str()) == default_account_id.as_deref() {
            0
        } else {
            1
        }
    });
    let mut seen = HashSet::new();
    let mut projected = Vec::new();
    for account in accounts {
        for namespace in BROWSER_SUPPORTED_EVM_CHAIN_NAMESPACES {
            let key = format!("{}:{}", namespace, account.address.to_ascii_lowercase());
            if !seen.insert(key) {
                continue;
            }
            let mut projected_account = account.clone();
            projected_account.chain_namespace = (*namespace).to_string();
            projected.push(projected_account);
        }
    }
    projected
}

pub(super) fn browser_wallet_account_is_signable_evm(account: &SystemWalletAccountSummary) -> bool {
    account.chain_namespace.starts_with("eip155:") && account.signing_available
}

pub(in crate::api::gateway) async fn browser_wallet_bridge_payload(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    authority: &RuntimeWalletAuthority,
    launch_token: Option<&str>,
    approval_origin: Option<&str>,
) -> serde_json::Value {
    let summary = system_wallet_accounts_summary(state, authority).await;
    let browser_accounts = browser_projected_evm_accounts(&summary);
    let browser_summary = SystemWalletAccountsSummary {
        accounts: browser_accounts,
        default_accounts: summary.default_accounts.clone(),
        ..summary.clone()
    };
    let default_chain_namespace = browser_default_chain_namespace(&browser_summary);
    let default_account_id = browser_default_account_id(&browser_summary);
    let approval_origin = browser_wallet_bridge_origin(approval_origin);
    let approval_url = browser_wallet_bridge_url(
        approval_origin.as_deref(),
        "/api/apps/browser/wallet/request-signature",
    );
    let transaction_url = browser_wallet_bridge_url(
        approval_origin.as_deref(),
        "/api/apps/browser/wallet/request-transaction",
    );
    let read_url =
        browser_wallet_bridge_url(approval_origin.as_deref(), "/api/apps/browser/wallet/read");
    let transaction_broadcast_url = browser_wallet_bridge_url(
        approval_origin.as_deref(),
        "/api/apps/browser/wallet/broadcast-transaction",
    );
    let approval_status_url = browser_wallet_bridge_url(
        approval_origin.as_deref(),
        "/api/apps/browser/wallet/approvals",
    );
    let bridge_url = browser_wallet_bridge_url(
        approval_origin.as_deref(),
        "/api/apps/browser/wallet/bridge",
    );
    serde_json::json!({
        "schema": "elastos.browser.wallet-bridge/v1",
        "principal_id": context.principal_id,
        "session_id": context.session_id,
        "default_chain_namespace": default_chain_namespace,
        "default_account_id": default_account_id,
        "accounts": browser_summary.accounts,
        "signing": "approval_required",
        "bridge_url": bridge_url,
        "approval_url": approval_url,
        "transaction_url": transaction_url,
        "read_url": read_url,
        "transaction_broadcast_url": transaction_broadcast_url,
        "approval_status_url": approval_status_url,
        "home_token": launch_token,
        "authority": "runtime_mediated",
    })
}

fn browser_wallet_bridge_url(origin: Option<&str>, path: &str) -> String {
    origin
        .map(|origin| format!("{origin}{path}"))
        .unwrap_or_else(|| path.to_string())
}

fn browser_wallet_bridge_origin(origin: Option<&str>) -> Option<String> {
    let origin = origin?.trim().trim_end_matches('/');
    let parsed = url::Url::parse(origin).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default()?;
    if browser_wallet_bridge_host_is_loopback(host) {
        return Some(format!("http://localhost:{port}"));
    }
    Some(origin.to_string())
}

fn browser_wallet_bridge_host_is_loopback(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
}

#[cfg(test)]
mod browser_wallet_bridge_tests {
    use super::*;

    fn account(chain_namespace: &str, account_id: &str) -> SystemWalletAccountSummary {
        SystemWalletAccountSummary {
            account_id: account_id.to_string(),
            chain_namespace: chain_namespace.to_string(),
            address: "0x1111111111111111111111111111111111111111".to_string(),
            proof_type: "managed_evm".to_string(),
            signing_available: true,
            signing_status: Some("managed_key_available".to_string()),
            label: None,
            connector_id: None,
            linked_at: 1,
        }
    }

    fn unavailable_account(chain_namespace: &str, account_id: &str) -> SystemWalletAccountSummary {
        SystemWalletAccountSummary {
            signing_available: false,
            signing_status: Some("managed_key_unavailable".to_string()),
            ..account(chain_namespace, account_id)
        }
    }

    fn default_account(
        chain_namespace: &str,
        account_id: &str,
        intent: &str,
    ) -> SystemWalletDefaultSummary {
        SystemWalletDefaultSummary {
            chain_namespace: chain_namespace.to_string(),
            intent: intent.to_string(),
            account_id: account_id.to_string(),
            set_at: 1,
        }
    }

    #[test]
    fn browser_default_chain_uses_runtime_wallet_default_not_host_policy() {
        let summary = SystemWalletAccountsSummary {
            available: true,
            linked_count: 2,
            accounts: vec![
                account("eip155:8453", "wallet:eip155:8453:0x111"),
                account("eip155:20", "wallet:eip155:20:0x222"),
            ],
            default_accounts: vec![default_account(
                "eip155:8453",
                "wallet:eip155:8453:0x111",
                "transaction_intent",
            )],
            note: None,
        };

        assert_eq!(
            browser_default_chain_namespace(&summary).as_deref(),
            Some("eip155:8453")
        );
    }

    #[test]
    fn browser_wallet_bridge_origin_rewrites_localhost_for_runtime_exit() {
        assert_eq!(
            browser_wallet_bridge_origin(Some("https://localhost:61180")).as_deref(),
            Some("http://localhost:61180")
        );
        assert_eq!(
            browser_wallet_bridge_origin(Some("http://127.0.0.1:8090")).as_deref(),
            Some("http://localhost:8090")
        );
        assert_eq!(
            browser_wallet_bridge_origin(Some("https://elastos.elacitylabs.com")).as_deref(),
            Some("https://elastos.elacitylabs.com")
        );
    }

    #[test]
    fn browser_default_chain_prefers_browser_connect_when_present() {
        let summary = SystemWalletAccountsSummary {
            available: true,
            linked_count: 2,
            accounts: vec![
                account("eip155:8453", "wallet:eip155:8453:0x111"),
                account("eip155:20", "wallet:eip155:20:0x222"),
            ],
            default_accounts: vec![
                default_account(
                    "eip155:8453",
                    "wallet:eip155:8453:0x111",
                    "transaction_intent",
                ),
                default_account("eip155:20", "wallet:eip155:20:0x222", "browser_connect"),
            ],
            note: None,
        };

        assert_eq!(
            browser_default_chain_namespace(&summary).as_deref(),
            Some("eip155:20")
        );
        assert_eq!(
            browser_default_account_id(&summary).as_deref(),
            Some("wallet:eip155:20:0x222")
        );
    }

    #[test]
    fn browser_default_account_tracks_wallet_transaction_default_identity() {
        let summary = SystemWalletAccountsSummary {
            available: true,
            linked_count: 2,
            accounts: vec![
                account("eip155:20", "wallet:eip155:20:0x111"),
                account("eip155:1", "wallet:eip155:1:0x222"),
            ],
            default_accounts: vec![default_account(
                "eip155:1",
                "wallet:eip155:1:0x222",
                "transaction_intent",
            )],
            note: None,
        };

        assert_eq!(
            browser_default_account_id(&summary).as_deref(),
            Some("wallet:eip155:1:0x222")
        );
        assert_eq!(
            browser_default_chain_namespace(&summary).as_deref(),
            Some("eip155:20")
        );
        let projected = browser_projected_evm_accounts(&summary);
        assert!(projected.iter().any(|account| {
            account.account_id == "wallet:eip155:1:0x222" && account.chain_namespace == "eip155:20"
        }));
    }

    #[test]
    fn browser_default_chain_skips_managed_accounts_that_cannot_sign() {
        let summary = SystemWalletAccountsSummary {
            available: true,
            linked_count: 2,
            accounts: vec![
                unavailable_account("eip155:20", "wallet:eip155:20:0x111"),
                account("eip155:8453", "wallet:eip155:8453:0x222"),
            ],
            default_accounts: vec![default_account(
                "eip155:20",
                "wallet:eip155:20:0x111",
                "browser_connect",
            )],
            note: None,
        };

        assert_eq!(
            browser_default_chain_namespace(&summary).as_deref(),
            Some("eip155:8453")
        );
    }

    #[test]
    fn browser_default_account_uses_latest_evm_transaction_default() {
        let summary = SystemWalletAccountsSummary {
            available: true,
            linked_count: 2,
            accounts: vec![
                account("eip155:1", "wallet:eip155:1:0x111"),
                account("eip155:20", "wallet:eip155:20:0x222"),
            ],
            default_accounts: vec![
                SystemWalletDefaultSummary {
                    set_at: 20,
                    ..default_account("eip155:20", "wallet:eip155:20:0x222", "transaction_intent")
                },
                SystemWalletDefaultSummary {
                    set_at: 10,
                    ..default_account("eip155:1", "wallet:eip155:1:0x111", "transaction_intent")
                },
            ],
            note: None,
        };

        assert_eq!(
            browser_default_account_id(&summary).as_deref(),
            Some("wallet:eip155:20:0x222")
        );
        assert_eq!(
            browser_default_chain_namespace(&summary).as_deref(),
            Some("eip155:20")
        );
    }
}
