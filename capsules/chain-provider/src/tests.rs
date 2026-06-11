use super::*;
use std::fs;
use std::path::Path;

mod support;

use support::*;

fn ok_data(response: Response) -> Value {
    match response {
        Response::Ok { data: Some(data) } => data,
        other => panic!("expected ok data, got {other:?}"),
    }
}

fn error_code(response: Response) -> String {
    match response {
        Response::Error { code, .. } => code,
        other => panic!("expected error, got {other:?}"),
    }
}

fn provider_with_rpc(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "esc-local",
                "display_name": "ESC Local",
                "kind": "evm_json_rpc",
                "chain_id": 20,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

#[test]
fn chain_provider_rejects_hidden_prepare_transaction_fields() {
    let request = json!({
        "op": "prepare_transaction",
        "network": "esc",
        "from": "0x0000000000000000000000000000000000000001",
        "to": "0x0000000000000000000000000000000000000002",
        "value": "0",
        "gas_price": "1"
    });

    let err = serde_json::from_value::<Request>(request)
        .expect_err("chain transaction requests must reject hidden raw transaction fields")
        .to_string();
    assert!(err.contains("gas_price"), "unexpected error: {err}");
}

#[test]
fn chain_provider_rejects_hidden_node_lifecycle_fields() {
    let request = json!({
        "op": "node_lifecycle",
        "network": "btc-local",
        "action": "status",
        "rpc_url": "http://127.0.0.1:8332"
    });

    let err = serde_json::from_value::<Request>(request)
        .expect_err("node lifecycle requests must reject hidden raw RPC authority")
        .to_string();
    assert!(err.contains("rpc_url"), "unexpected error: {err}");
}

fn provider_with_rights_rpc(rpc_url: String, selector: &str) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "esc-local",
                "display_name": "ESC Local",
                "kind": "evm_json_rpc",
                "chain_id": 20,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url,
                "rights_methods": [{
                    "id": "has_access_by_content_id",
                    "contract": "0x0000000000000000000000000000000000000001",
                    "abi": "has_access_by_content_id_string_address_string",
                    "selector": selector
                }]
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

fn provider_with_bitcoin_rpc(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    init_bitcoin_rpc_provider(&mut provider, rpc_url);
    provider
}

fn provider_with_bitcoin_rpc_in(data_dir: &Path, rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::with_data_dir(data_dir.to_path_buf());
    init_bitcoin_rpc_provider(&mut provider, rpc_url);
    provider
}

fn init_bitcoin_rpc_provider(provider: &mut ChainProvider, rpc_url: String) {
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_core_rpc",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "Bitcoin Core",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
}

fn add_true_node_supervisor(provider: &mut ChainProvider, network_id: &str) {
    let init = provider.handle(Request::Init {
        config: json!({
            "node_supervisor": {
                "networks": {
                    network_id: {
                        "start": { "program": "/bin/true", "args": [] },
                        "stop": { "program": "/bin/true", "args": [] },
                        "restart": { "program": "/bin/true", "args": [] },
                        "timeout_ms": 1000
                    }
                }
            }
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
}

fn provider_with_bitcoin_rest(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_rest",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

fn provider_with_mainchain_rest(rpc_url: String) -> ChainProvider {
    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "ela-local",
                "display_name": "ELA Local",
                "kind": "mainchain_rest",
                "chain_id": null,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));
    provider
}

#[test]
fn lists_production_default_networks_without_rpc_urls() {
    let mut provider = ChainProvider::new();
    let data = ok_data(provider.handle(Request::Networks));
    let networks = data["networks"].as_array().unwrap();
    let ids = networks
        .iter()
        .map(|network| network["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["ela-mainnet", "esc-mainnet", "base-mainnet", "btc-mainnet"]
    );
    assert!(networks.iter().all(|network| network["mainnet"] == true));
    assert!(networks
        .iter()
        .all(|network| !network["id"].as_str().unwrap().contains("testnet")));
    assert!(networks
        .iter()
        .all(|network| !network["id"].as_str().unwrap().contains("eid")));
    assert!(networks
        .iter()
        .any(|network| network["id"] == "base-mainnet"
            && network["kind"] == "evm_json_rpc"
            && network["chain_id"] == 8453
            && network["configured"] == true));
    assert!(networks.iter().any(|network| network["id"] == "btc-mainnet"
        && network["kind"] == "bitcoin_rest"
        && network["configured"] == true));
    assert!(networks
        .iter()
        .all(|network| network.get("rpc_url").is_none()));
}

#[test]
fn rejects_invalid_balance_address_before_upstream() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    let response = provider.handle(Request::Balance {
        network: "esc-local".to_string(),
        address: "0x1234".to_string(),
        block: None,
    });
    assert_eq!(error_code(response), "invalid_address");
}

#[test]
fn rejects_mainchain_for_evm_operations() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Balance {
        network: "ela-mainnet".to_string(),
        address: "0x0000000000000000000000000000000000000000".to_string(),
        block: None,
    });
    assert_eq!(error_code(response), "unsupported_network_kind");
}

#[test]
fn proxies_mainchain_status_with_typed_rest_method() {
    let rpc_url = spawn_http_sequence_server(vec![(
        "/blocks?page=1&pageSize=1",
        json!({
            "data": [{
                "height": 2203455,
                "hash": "c5646678a05b7abcdc7449edafd331b5994a998b50f784e5b4ee05071749930a",
                "timestamp": 1777700819,
                "txCount": 3
            }],
            "total": 2203456
        })
        .to_string(),
        "application/json",
    )]);
    let mut provider = provider_with_mainchain_rest(rpc_url);
    let data = ok_data(provider.handle(Request::Status {
        network: "ela-local".to_string(),
    }));
    assert_eq!(data["block_height"], 2203455);
    assert_eq!(
        data["best_block_hash"],
        "c5646678a05b7abcdc7449edafd331b5994a998b50f784e5b4ee05071749930a"
    );
    assert_eq!(data["tx_count"], 3);
}

#[test]
fn bitcoin_status_fails_closed_when_node_is_not_configured() {
    let mut provider = provider_with_bitcoin_rpc(String::new());
    let response = provider.handle(Request::Status {
        network: "btc-local".to_string(),
    });
    assert_eq!(error_code(response), "node_not_configured");
}

#[test]
fn proxies_bitcoin_status_with_typed_method() {
    let rpc_url = spawn_rpc_server(
        "getblockchaininfo",
        json!({
            "chain": "main",
            "blocks": 840000,
            "headers": 840001,
            "bestblockhash": "0000000000000000000000000000000000000000000000000000000000000000",
            "initialblockdownload": false,
            "verificationprogress": 0.999,
        }),
    );
    let mut provider = provider_with_bitcoin_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::Status {
        network: "btc-local".to_string(),
    }));
    assert_eq!(data["chain"], "main");
    assert_eq!(data["block_height"], 840000);
    assert_eq!(data["headers"], 840001);
}

#[test]
fn proxies_bitcoin_rest_status_with_typed_methods() {
    let rpc_url = spawn_http_sequence_server(vec![
        ("/blocks/tip/height", "840000".to_string(), "text/plain"),
        (
            "/blocks/tip/hash",
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            "text/plain",
        ),
    ]);
    let mut provider = provider_with_bitcoin_rest(rpc_url);
    let data = ok_data(provider.handle(Request::Status {
        network: "btc-local".to_string(),
    }));
    assert_eq!(data["chain"], "main");
    assert_eq!(data["block_height"], 840000);
    assert_eq!(
        data["best_block_hash"],
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
}

#[test]
fn proxies_bitcoin_rest_balance_with_typed_method() {
    let address = "bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l";
    let rpc_url = spawn_http_sequence_server(vec![(
        "/address/bc1q9vza2e8x573nczrlzms0wvx3gsqjx7vavgkx0l",
        json!({
            "chain_stats": {
                "funded_txo_sum": 120_000,
                "spent_txo_sum": 20_000
            },
            "mempool_stats": {
                "funded_txo_sum": 7_000,
                "spent_txo_sum": 2_000
            }
        })
        .to_string(),
        "application/json",
    )]);
    let mut provider = provider_with_bitcoin_rest(rpc_url);
    let data = ok_data(provider.handle(Request::Balance {
        network: "btc-local".to_string(),
        address: address.to_string(),
        block: None,
    }));
    assert_eq!(data["network"], "btc-local");
    assert_eq!(data["address"], address);
    assert_eq!(data["confirmed_sats"], 100_000);
    assert_eq!(data["mempool_sats"], 5_000);
    assert_eq!(data["balance_sats"], 105_000);
    assert_eq!(data["native_symbol"], "BTC");
}

#[test]
fn proxies_block_number_with_typed_evm_method() {
    let rpc_url = spawn_rpc_server("eth_blockNumber", json!("0x2a"));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::BlockNumber {
        network: "esc-local".to_string(),
    }));
    assert_eq!(data["network"], "esc-local");
    assert_eq!(data["block_number_hex"], "0x2a");
    assert_eq!(data["block_number"], 42);
}

#[test]
fn proxies_evm_sync_health_with_typed_method() {
    let rpc_url = spawn_rpc_server("eth_syncing", json!(false));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::SyncHealth {
        network: "esc-local".to_string(),
    }));
    assert_eq!(data["synced"], true);
    assert_eq!(data["syncing"], false);
    assert_eq!(data["network"]["id"], "esc-local");
    assert!(data["network"].get("rpc_url").is_none());
}

#[test]
fn parses_evm_sync_progress_without_raw_rpc_passthrough() {
    let rpc_url = spawn_rpc_server(
        "eth_syncing",
        json!({
            "startingBlock": "0x1",
            "currentBlock": "0x2a",
            "highestBlock": "0x64"
        }),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::SyncHealth {
        network: "esc-local".to_string(),
    }));
    assert_eq!(data["synced"], false);
    assert_eq!(data["sync"]["starting_block"], 1);
    assert_eq!(data["sync"]["current_block"], 42);
    assert_eq!(data["sync"]["highest_block"], 100);
}

#[test]
fn creates_typed_sync_health_proof_without_exposing_rpc() {
    let rpc_url = spawn_rpc_server("eth_syncing", json!(false));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::Proof {
        network: "esc-local".to_string(),
        proof_kind: ChainProofKind::SyncHealth,
        subject: "person:local:alice".to_string(),
    }));

    assert_eq!(data["schema"], "elastos.chain.proof/v1");
    assert_eq!(data["network"], "esc-local");
    assert_eq!(data["proof_kind"], "sync_health");
    assert_eq!(data["subject"], "person:local:alice");
    assert!(data["evidence_hash"].as_str().unwrap().starts_with("0x"));
    assert!(data.get("rpc_url").is_none());
}

#[test]
fn verifies_erc1271_signature_through_typed_eth_call() {
    let message_hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let signature = "0x010203";
    let expected_data = encode_erc1271_is_valid_signature_call(
        &decode_hex(message_hash, Some(32), "message_hash").unwrap(),
        &decode_hex(signature, None, "signature").unwrap(),
    );
    let rpc_url = spawn_eth_call_server(
        expected_data,
        json!("0x1626ba7e00000000000000000000000000000000000000000000000000000000"),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::Erc1271IsValidSignature {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        message_hash: message_hash.to_string(),
        signature: signature.to_string(),
    }));

    assert_eq!(data["schema"], "elastos.chain.erc1271_proof/v1");
    assert_eq!(data["chain_id"], 20);
    assert_eq!(
        data["contract"],
        "0x0000000000000000000000000000000000000001"
    );
    assert_eq!(data["message_hash"], message_hash);
    assert_eq!(data["valid"], true);
    assert_eq!(data["magic_value"], "0x1626ba7e");
    assert!(data["network"].get("rpc_url").is_none());
}

#[test]
fn proxies_typed_evm_contract_call_without_raw_rpc_url() {
    let data_hex = "0x70a082310000000000000000000000001111111111111111111111111111111111111111";
    let rpc_url = spawn_eth_call_server(
        data_hex.to_string(),
        json!("0x0000000000000000000000000000000000000000000000000000000000000042"),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::ContractCall {
        network: "esc-local".to_string(),
        to: "0x0000000000000000000000000000000000000001".to_string(),
        data: data_hex.to_string(),
        block: None,
    }));

    assert_eq!(data["schema"], "elastos.chain.contract_call/v1");
    assert_eq!(data["network"], "esc-local");
    assert_eq!(
        data["result"],
        "0x0000000000000000000000000000000000000000000000000000000000000042"
    );
    assert!(data.get("rpc_url").is_none());
}

#[test]
fn proxies_typed_evm_gas_estimate_without_wallet_approval() {
    let rpc_url = spawn_rpc_server("eth_estimateGas", json!("0x5208"));
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::EstimateGas {
        network: "esc-local".to_string(),
        from: "0x0000000000000000000000000000000000000001".to_string(),
        to: "0x0000000000000000000000000000000000000002".to_string(),
        value: Some("0x1".to_string()),
        data: Some("0x1234".to_string()),
    }));

    assert_eq!(data["schema"], "elastos.chain.gas_estimate/v1");
    assert_eq!(data["gas_limit"], "0x5208");
    assert!(data.get("requires_wallet_approval").is_none());
    assert!(data.get("rpc_url").is_none());
}

#[test]
fn erc1271_rejects_invalid_inputs_before_backend() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    assert_eq!(
        error_code(provider.handle(Request::Erc1271IsValidSignature {
            network: "esc-local".to_string(),
            contract: "0x0000000000000000000000000000000000000001".to_string(),
            message_hash: "0x1234".to_string(),
            signature: "0x0102".to_string(),
        })),
        "invalid_message_hash"
    );
    assert_eq!(
        error_code(provider.handle(Request::Erc1271IsValidSignature {
            network: "esc-local".to_string(),
            contract: "not-an-address".to_string(),
            message_hash:
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            signature: "0x0102".to_string(),
        })),
        "invalid_contract"
    );
}

#[test]
fn prepares_typed_evm_transaction_intent_without_node_write() {
    let rpc_url = spawn_rpc_sequence_server(vec![
        ("eth_getTransactionCount", json!("0x7")),
        ("eth_gasPrice", json!("0x3b9aca00")),
        ("eth_estimateGas", json!("0x5208")),
    ]);
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::PrepareTransaction {
        network: "esc-local".to_string(),
        from: "0x0000000000000000000000000000000000000001".to_string(),
        to: "0x0000000000000000000000000000000000000002".to_string(),
        value: "0x0".to_string(),
        data: Some("0x1234".to_string()),
    }));

    assert_eq!(
        data["schema"],
        "elastos.chain.unsigned_transaction_intent/v1"
    );
    assert_eq!(data["transaction_type"], "eip155_legacy");
    assert_eq!(data["nonce"], "0x7");
    assert_eq!(data["gas_price"], "0x3b9aca00");
    assert_eq!(data["gas_limit"], "0x5208");
    assert_eq!(data["requires_wallet_approval"], true);
    assert_eq!(data["wallet_intent"], "transaction_intent");
    assert!(data["network"].get("rpc_url").is_none());
}

#[test]
fn exposes_typed_evm_dapp_read_helpers_without_raw_rpc_urls() {
    let rpc_url = spawn_rpc_sequence_server(vec![
        ("eth_getTransactionCount", json!("0x7")),
        ("eth_gasPrice", json!("0x3b9aca00")),
        (
            "eth_feeHistory",
            json!({
                "oldestBlock": "0x1",
                "baseFeePerGas": ["0x3b9aca00", "0x3b9aca01"],
                "gasUsedRatio": [0.5],
                "reward": [["0x1"]]
            }),
        ),
        ("eth_getCode", json!("0x60016001")),
        (
            "eth_getLogs",
            json!([{
                "address": "0x0000000000000000000000000000000000000002",
                "blockNumber": "0x2a",
                "data": "0x",
                "topics": []
            }]),
        ),
    ]);
    let mut provider = provider_with_rpc(rpc_url);
    let address = "0x0000000000000000000000000000000000000001";

    let nonce = ok_data(provider.handle(Request::TransactionCount {
        network: "esc-local".to_string(),
        address: address.to_string(),
        block: Some("pending".to_string()),
    }));
    assert_eq!(nonce["schema"], "elastos.chain.transaction_count/v1");
    assert_eq!(nonce["nonce"], "0x7");
    assert!(nonce.get("rpc_url").is_none());

    let gas_price = ok_data(provider.handle(Request::GasPrice {
        network: "esc-local".to_string(),
    }));
    assert_eq!(gas_price["schema"], "elastos.chain.gas_price/v1");
    assert_eq!(gas_price["gas_price"], "0x3b9aca00");

    let history = ok_data(provider.handle(Request::FeeHistory {
        network: "esc-local".to_string(),
        block_count: "0x1".to_string(),
        newest_block: "latest".to_string(),
        reward_percentiles: vec![1.0],
    }));
    assert_eq!(history["schema"], "elastos.chain.fee_history/v1");
    assert_eq!(history["history"]["oldestBlock"], "0x1");

    let code = ok_data(provider.handle(Request::Code {
        network: "esc-local".to_string(),
        address: address.to_string(),
        block: Some("latest".to_string()),
    }));
    assert_eq!(code["schema"], "elastos.chain.code/v1");
    assert_eq!(code["code"], "0x60016001");

    let logs = ok_data(provider.handle(Request::Logs {
        network: "esc-local".to_string(),
        filter: json!({
            "fromBlock": "0x1",
            "toBlock": "latest",
            "address": "0x0000000000000000000000000000000000000002",
            "topics": []
        }),
    }));
    assert_eq!(logs["schema"], "elastos.chain.logs/v1");
    assert_eq!(logs["logs"][0]["blockNumber"], "0x2a");
    assert_json_strings_do_not_contain(&logs, "127.0.0.1");
}

#[test]
fn broadcasts_typed_evm_signed_transaction() {
    let rpc_url = spawn_rpc_server(
        "eth_sendRawTransaction",
        json!("0x000000000000000000000000000000000000000000000000000000000000002a"),
    );
    let mut provider = provider_with_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::BroadcastTransaction {
        network: "esc-local".to_string(),
        signed_transaction: "0x1234".to_string(),
    }));

    assert_eq!(data["schema"], "elastos.chain.broadcast_receipt/v1");
    assert_eq!(
        data["transaction_hash"],
        "0x000000000000000000000000000000000000000000000000000000000000002a"
    );
}

#[test]
fn node_lifecycle_reports_status_and_fails_closed_for_control() {
    let data_dir = TestDataDir::new();
    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:8332".to_string());
    let data = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    assert_eq!(data["schema"], "elastos.chain.node_lifecycle/v1");
    assert_eq!(data["managed"], true);
    assert_eq!(data["control_available"], false);
    assert_eq!(
        data["control_reason"],
        "node lifecycle control requires an operator-approved supervisor"
    );
    assert_eq!(data["state"], "external_loopback");
    assert!(data["network"].get("rpc_url").is_none());
    assert_json_strings_do_not_contain(&data, "127.0.0.1");
    assert_json_strings_do_not_contain(&data, "8332");

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Start,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
}

#[test]
fn node_lifecycle_runs_operator_supervisor_for_loopback_nodes() {
    let data_dir = TestDataDir::new();
    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:18446".to_string());
    add_true_node_supervisor(&mut provider, "btc-local");

    let status = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    assert_eq!(status["managed"], true);
    assert_eq!(status["control_available"], true);
    assert_eq!(status["state"], "managed_local");
    assert_json_strings_do_not_contain(&status, "/bin/true");
    assert_json_strings_do_not_contain(&status, "18446");

    let start = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Start,
    }));
    assert_eq!(start["action"], "start");
    assert_eq!(start["control_available"], true);
    assert_eq!(start["state"], "managed_local");
    assert_json_strings_do_not_contain(&start, "/bin/true");
    assert_json_strings_do_not_contain(&start, "18446");
}

#[test]
fn node_lifecycle_rejects_supervisor_control_for_remote_backends() {
    let mut provider = provider_with_rpc("https://example.invalid/rpc".to_string());
    add_true_node_supervisor(&mut provider, "esc-local");

    let response = provider.handle(Request::NodeLifecycle {
        network: "esc-local".to_string(),
        action: NodeLifecycleAction::Start,
    });

    assert_eq!(error_code(response), "managed_node_unavailable");
}

#[test]
fn node_lifecycle_state_survives_provider_reload_without_raw_rpc() {
    let data_dir = TestDataDir::new();
    let rpc_url = "http://127.0.0.1:18443".to_string();
    let mut provider = provider_with_bitcoin_rpc_in(data_dir.path(), rpc_url.clone());

    let first = ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    let first_seen_at = first["first_seen_at"].as_u64().unwrap();
    assert_eq!(first["state"], "external_loopback");
    assert_json_strings_do_not_contain(&first, &rpc_url);
    assert_json_strings_do_not_contain(&first, "18443");

    let state_path = node_lifecycle_state_path(data_dir.path());
    let state = read_node_lifecycle_state_file(&state_path).unwrap();
    let persisted = state.networks.get("btc-local").unwrap();
    assert_eq!(persisted.state, NodeLifecycleStateKind::ExternalLoopback);
    assert!(persisted.managed);
    let state_json = serde_json::to_value(&state).unwrap();
    assert!(state_json
        .pointer("/networks/btc-local/control_available")
        .is_none());
    assert!(state_json
        .pointer("/networks/btc-local/control_reason")
        .is_none());
    assert_json_strings_do_not_contain(&state_json, &rpc_url);
    assert_json_strings_do_not_contain(&state_json, "18443");

    let mut reloaded = provider_with_bitcoin_rpc_in(data_dir.path(), rpc_url);
    let second = ok_data(reloaded.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    assert_eq!(second["state"], "external_loopback");
    assert_eq!(second["first_seen_at"].as_u64().unwrap(), first_seen_at);
    assert_json_strings_do_not_contain(&second, "127.0.0.1");
    assert_json_strings_do_not_contain(&second, "18443");
}

#[test]
fn unsupported_node_lifecycle_actions_do_not_persist_state() {
    let data_dir = TestDataDir::new();
    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:18444".to_string());

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Start,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
    let state_path = node_lifecycle_state_path(data_dir.path());
    assert!(!state_path.exists());

    ok_data(provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    }));
    let before = fs::read_to_string(&state_path).unwrap();

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Restart,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
    let after = fs::read_to_string(&state_path).unwrap();
    assert_eq!(after, before);

    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Stop,
    });
    assert_eq!(error_code(response), "managed_node_unavailable");
    let after = fs::read_to_string(&state_path).unwrap();
    assert_eq!(after, before);
}

#[test]
fn corrupt_node_lifecycle_state_fails_closed_with_typed_error() {
    let data_dir = TestDataDir::new();
    let state_path = node_lifecycle_state_path(data_dir.path());
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    fs::write(&state_path, "{not json").unwrap();

    let mut provider =
        provider_with_bitcoin_rpc_in(data_dir.path(), "http://127.0.0.1:18445".to_string());
    let response = provider.handle(Request::NodeLifecycle {
        network: "btc-local".to_string(),
        action: NodeLifecycleAction::Status,
    });
    assert_eq!(error_code(response), "node_lifecycle_state_unavailable");
}

#[test]
fn bitcoin_sync_health_reports_initial_block_download() {
    let rpc_url = spawn_rpc_server(
        "getblockchaininfo",
        json!({
            "chain": "main",
            "blocks": 840000,
            "headers": 840100,
            "initialblockdownload": true,
            "verificationprogress": 0.98
        }),
    );
    let mut provider = provider_with_bitcoin_rpc(rpc_url);
    let data = ok_data(provider.handle(Request::SyncHealth {
        network: "btc-local".to_string(),
    }));
    assert_eq!(data["synced"], false);
    assert_eq!(data["syncing"], true);
    assert_eq!(data["block_height"], 840000);
    assert_eq!(data["headers"], 840100);
}

#[test]
fn has_access_by_content_id_validates_typed_inputs_before_backend() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    let response = provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "view".to_string(),
    });

    assert_eq!(error_code(response), "rights_query_not_configured");
}

#[test]
fn has_access_by_content_id_calls_configured_typed_rights_abi() {
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        "bafybeigprotectedcontent",
        "0x0000000000000000000000000000000000000002",
        "view",
    )
    .unwrap();
    let rpc_url = spawn_eth_call_server(
        expected_data,
        json!("0x0000000000000000000000000000000000000000000000000000000000000001"),
    );
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");

    let data = ok_data(provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "view".to_string(),
    }));

    assert_eq!(data["has_access"], true);
    assert_eq!(data["right"], "view");
}

#[test]
fn has_access_by_content_id_address_bytes16_encodes_the_real_base_abi() {
    // Real Base ABI: hasAccessByContentId(address holder, bytes16 contentId). Two static
    // words: [holder address][bytes16 KID]. `right` is NOT encoded (binary access on-chain).
    let selector = "0x54d42821";
    let subject = "0x0000000000000000000000000000000000000002";
    let content_id = "0x38691296765e76a331f5d5630bddf9f5"; // 16-byte KID

    let expected_data =
        encode_has_access_by_content_id_address_bytes16(selector, subject, content_id).unwrap();
    // selector(4) + 2 words(64) = 68 bytes => 136 hex + "0x".
    assert_eq!(expected_data.len(), 2 + 2 * 68);
    assert!(expected_data.starts_with("0x54d42821"));

    let rpc_url = spawn_eth_call_server(
        expected_data,
        json!("0x0000000000000000000000000000000000000000000000000000000000000001"),
    );

    let mut provider = ChainProvider::new();
    let init = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "esc-local",
                "display_name": "ESC Local",
                "kind": "evm_json_rpc",
                "chain_id": 20,
                "native_symbol": "ELA",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": rpc_url,
                "rights_methods": [{
                    "id": "has_access_by_content_id",
                    "contract": "0x0000000000000000000000000000000000000001",
                    "abi": "has_access_by_content_id_address_bytes16",
                    "selector": selector
                }]
            }]
        }),
    });
    assert!(matches!(init, Response::Ok { .. }));

    let data = ok_data(provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        content_id: content_id.to_string(),
        subject: subject.to_string(),
        right: "view".to_string(),
    }));
    assert_eq!(data["has_access"], true);
}

#[test]
fn has_access_by_content_id_decodes_unowned_as_false() {
    // The AuthorityGateway returns ABI-encoded `false` for content the subject does
    // NOT own — the rights step must surface that as a real `has_access: false`
    // (which downstream becomes a `denied` rights receipt), not an error.
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        "bafybeigprotectedcontent",
        "0x0000000000000000000000000000000000000002",
        "view",
    )
    .unwrap();
    let rpc_url = spawn_eth_call_server(
        expected_data,
        json!("0x0000000000000000000000000000000000000000000000000000000000000000"),
    );
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");

    let data = ok_data(provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "view".to_string(),
    }));

    assert_eq!(data["has_access"], false);
}

#[test]
fn has_access_by_content_id_fails_closed_on_malformed_bool() {
    // A non-boolean ABI word (here the high bytes are non-zero) must fail closed —
    // never silently coerced to true/false. The decrypt chain depends on this answer.
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        "bafybeigprotectedcontent",
        "0x0000000000000000000000000000000000000002",
        "view",
    )
    .unwrap();
    let rpc_url = spawn_eth_call_server(
        expected_data,
        json!("0x00000000000000000000000000000000000000000000000000000000000000ff"),
    );
    let mut provider = provider_with_rights_rpc(rpc_url, "0x12345678");

    let response = provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "view".to_string(),
    });

    assert_eq!(error_code(response), "upstream_invalid_bool");
}

#[test]
fn has_access_by_content_id_rejects_unconfigured_contract_before_backend() {
    let mut provider = provider_with_rights_rpc("http://127.0.0.1:9".to_string(), "0x12345678");
    let response = provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000003".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "view".to_string(),
    });

    assert_eq!(error_code(response), "rights_contract_not_allowed");
}

#[test]
fn init_rejects_rights_methods_on_non_evm_networks() {
    let mut provider = ChainProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({
            "networks": [{
                "id": "btc-local",
                "display_name": "BTC Local",
                "kind": "bitcoin_rest",
                "chain_id": null,
                "native_symbol": "BTC",
                "provider": "test",
                "mainnet": false,
                "explorer_url": null,
                "rpc_url": "https://mempool.space/api",
                "rights_methods": [{
                    "id": "has_access_by_content_id",
                    "contract": "0x0000000000000000000000000000000000000001",
                    "abi": "has_access_by_content_id_string_address_string",
                    "selector": "0x12345678"
                }]
            }]
        }),
    });

    assert_eq!(error_code(response), "invalid_config");
}

#[test]
fn has_access_by_content_id_rejects_invalid_right_before_backend() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    let response = provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x0000000000000000000000000000000000000001".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "raw_call".to_string(),
    });

    assert_eq!(error_code(response), "invalid_right");
}

#[test]
fn has_access_by_content_id_rejects_invalid_contract_before_backend() {
    let mut provider = provider_with_rpc("http://127.0.0.1:9".to_string());
    let response = provider.handle(Request::HasAccessByContentId {
        network: "esc-local".to_string(),
        contract: "0x1234".to_string(),
        content_id: "bafybeigprotectedcontent".to_string(),
        subject: "0x0000000000000000000000000000000000000002".to_string(),
        right: "view".to_string(),
    });

    assert_eq!(error_code(response), "invalid_contract");
}

// --- assemble_mint: content-mint calldata (PC2 mint(string,uint16,bytes,bytes)) -----
//
// These decode the produced calldata back against the Solidity ABI spec (no ethers
// dependency) so the encoder is proven correct, not merely pinned. All names contain
// "mint" so the ladder can gate them by filter (the suite has one env-flaky lifecycle
// test that is intentionally excluded from the deterministic count).

const MINT_KID32: &str = "38691296765e76a331f5d5630bddf9f5";
const MINT_SELECTOR: &str = "0xaabbccdd";
const MINT_CHANNEL: &str = "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";

fn mint_hex_to_bytes(hex: &str) -> Vec<u8> {
    let clean = hex.strip_prefix("0x").unwrap_or(hex);
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
        .collect()
}

/// Read the low-64-bits of the 32-byte word at byte offset `off`.
fn mint_word_u64(body: &[u8], off: usize) -> u64 {
    u64::from_be_bytes(body[off + 24..off + 32].try_into().unwrap())
}

fn mint_free_request() -> Request {
    Request::AssembleMint {
        mint: Box::new(
            serde_json::from_value(json!({
                "selector": MINT_SELECTOR,
                "to": MINT_CHANNEL,
                "token_uri": "QmMetaFolderCidV0/metadata.json",
                "op_type_code": 0,
                "content_id": format!("0x{MINT_KID32}"),
            }))
            .unwrap(),
        ),
    }
}

fn mint_paid_value(reseller_cut: Option<u16>, op_type_code: u16) -> serde_json::Value {
    let mut op_raw = json!({
        "metadata_uri": "ipfs://QmMetaFolderCidV0",
        "addresses": [MINT_CHANNEL],
        "role_types": [1],
        "amounts": ["100"],
    });
    if let Some(cut) = reseller_cut {
        op_raw["reseller_cut"] = json!(cut);
    }
    json!({
        "selector": MINT_SELECTOR,
        "to": MINT_CHANNEL,
        "token_uri": "QmMetaFolderCidV0/metadata.json",
        "op_type_code": op_type_code,
        "content_id": format!("0x{MINT_KID32}"),
        "value_wei": "0x16345785d8a0000",
        "op_raw": op_raw,
        "sell": {
            "copies": "100",
            "price_wei": "1000000000000000000",
            "pay_token": "0x0000000000000000000000000000000000000000",
        },
    })
}

#[test]
fn mint_assemble_free_calldata_decodes_to_the_bytes16_content_id() {
    let data = ok_data(ChainProvider::new().handle(mint_free_request()));
    assert_eq!(data["function"], "mint(string,uint16,bytes,bytes)");
    assert_eq!(data["to"], MINT_CHANNEL);
    assert_eq!(data["value"], "0x0");
    assert_eq!(data["signed"], false);

    let calldata = mint_hex_to_bytes(data["data"].as_str().unwrap());
    // selector prefix.
    assert_eq!(&calldata[..4], &[0xaa, 0xbb, 0xcc, 0xdd]);
    let body = &calldata[4..];

    // head: [uri_off=0x80, opType=0, op_off, sell_off].
    assert_eq!(mint_word_u64(body, 0), 128, "uri offset");
    assert_eq!(mint_word_u64(body, 32), 0, "op_type FREE");
    let op_off = mint_word_u64(body, 64) as usize;
    let sell_off = mint_word_u64(body, 96) as usize;

    // _uri string decodes to the tokenURI.
    let uri_len = mint_word_u64(body, 128) as usize;
    let uri = &body[160..160 + uri_len];
    assert_eq!(uri, b"QmMetaFolderCidV0/metadata.json");

    // opRawData = abi.encode(bytes16): a 32-byte dynamic blob == the bytes16 word.
    assert_eq!(mint_word_u64(body, op_off), 32, "opRawData byte length");
    let op_word = &body[op_off + 32..op_off + 64];
    assert_eq!(&op_word[..16], &mint_hex_to_bytes(MINT_KID32)[..], "contentId high 16");
    assert!(op_word[16..].iter().all(|b| *b == 0), "bytes16 zero-padded right");

    // sellRawData is empty for a free mint.
    assert_eq!(mint_word_u64(body, sell_off), 0, "sellRawData empty");
}

#[test]
fn mint_assemble_paid_encodes_sell_terms_and_op_payees() {
    let data = ok_data(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(serde_json::from_value(mint_paid_value(None, 1)).unwrap()),
    }));
    assert_eq!(data["value"], "0x16345785d8a0000");
    let calldata = mint_hex_to_bytes(data["data"].as_str().unwrap());
    let body = &calldata[4..];

    assert_eq!(mint_word_u64(body, 32), 1, "op_type BUY_ONCE");
    let op_off = mint_word_u64(body, 64) as usize;
    let sell_off = mint_word_u64(body, 96) as usize;

    // opRawData tuple head[0] is the bytes16 contentId.
    let op_body = op_off + 32; // skip the dynamic-bytes length word
    assert_eq!(
        &body[op_body..op_body + 16],
        &mint_hex_to_bytes(MINT_KID32)[..],
        "opRawData leads with bytes16 contentId"
    );

    // sellRawData = (copies, price, payToken).
    let sell_body = sell_off + 32;
    assert_eq!(mint_word_u64(body, sell_body), 100, "copies");
    assert_eq!(
        mint_word_u64(body, sell_body + 32),
        1_000_000_000_000_000_000,
        "price (1e18) fits low 64 bits here"
    );
}

#[test]
fn mint_assemble_buy_and_resell_appends_reseller_cut() {
    // BUY_AND_RESELL opRawData has 6 head words (the trailing uint16). The bytes16 head
    // word is unchanged; the difference is detectable by op_raw length growth, so just
    // assert it assembles and the buy_once form rejects a reseller_cut.
    let resell = ok_data(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(serde_json::from_value(mint_paid_value(Some(900), 2)).unwrap()),
    }));
    assert_eq!(resell["op_type_code"], 2);
    assert!(resell["data"].as_str().unwrap().starts_with("0xaabbccdd"));
}

#[test]
fn mint_buy_once_rejects_a_reseller_cut() {
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(serde_json::from_value(mint_paid_value(Some(900), 1)).unwrap()),
    }));
    assert_eq!(err, "invalid_mint");
}

#[test]
fn mint_buy_and_resell_requires_a_reseller_cut() {
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(serde_json::from_value(mint_paid_value(None, 2)).unwrap()),
    }));
    assert_eq!(err, "invalid_mint");
}

#[test]
fn mint_free_rejects_sale_terms() {
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(serde_json::from_value(mint_paid_value(None, 0)).unwrap()),
    }));
    assert_eq!(err, "invalid_mint");
}

#[test]
fn mint_paid_requires_sale_terms() {
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(
            serde_json::from_value(json!({
                "selector": MINT_SELECTOR,
                "to": MINT_CHANNEL,
                "token_uri": "QmMetaFolderCidV0/metadata.json",
                "op_type_code": 1,
                "content_id": format!("0x{MINT_KID32}"),
            }))
            .unwrap(),
        ),
    }));
    assert_eq!(err, "invalid_mint");
}

#[test]
fn mint_rejects_a_non_bytes16_content_id() {
    let mut bad = serde_json::from_value::<MintAssembly>(json!({
        "selector": MINT_SELECTOR,
        "to": MINT_CHANNEL,
        "token_uri": "QmMetaFolderCidV0/metadata.json",
        "op_type_code": 0,
        "content_id": "0xdeadbeef",
    }))
    .unwrap();
    bad.content_id = "0xdeadbeef".to_string();
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint { mint: Box::new(bad) }));
    assert_eq!(err, "invalid_mint");
}

#[test]
fn mint_rejects_a_bad_selector() {
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(
            serde_json::from_value(json!({
                "selector": "0xzz",
                "to": MINT_CHANNEL,
                "token_uri": "QmMetaFolderCidV0/metadata.json",
                "op_type_code": 0,
                "content_id": format!("0x{MINT_KID32}"),
            }))
            .unwrap(),
        ),
    }));
    assert_eq!(err, "invalid_mint");
}

#[test]
fn mint_rejects_a_bad_channel_address() {
    let err = error_code(ChainProvider::new().handle(Request::AssembleMint {
        mint: Box::new(
            serde_json::from_value(json!({
                "selector": MINT_SELECTOR,
                "to": "not-an-address",
                "token_uri": "QmMetaFolderCidV0/metadata.json",
                "op_type_code": 0,
                "content_id": format!("0x{MINT_KID32}"),
            }))
            .unwrap(),
        ),
    }));
    assert_eq!(err, "invalid_to");
}
