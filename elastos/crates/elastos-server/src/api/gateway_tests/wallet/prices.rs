use super::super::*;

#[test]
fn test_wallet_prices_parse_coingecko_payload() {
    let payload = json!({
        "bitcoin": { "usd": 67800.12, "usd_24h_change": 1.84 },
        "ethereum": { "usd": 3650.40, "usd_24h_change": 0.92 },
        "elastos": { "usd": 2.50, "usd_24h_change": -0.31 },
        "usd-coin": { "usd": 1.0, "usd_24h_change": 0.01 }
    });
    let prices = wallet_prices_from_coingecko_payload(&payload, 1_731_000_000).unwrap();
    assert_eq!(prices["BTC"].usd, 67800.12);
    assert_eq!(prices["ETH"].change_24h, 0.92);
    assert_eq!(prices["ELA"].change_24h, -0.31);
    assert_eq!(prices["USDC"].usd, 1.0);
}

#[test]
fn test_wallet_price_http_source_requires_explicit_approval() {
    assert!(wallet_price_source_decision(|_| None, || None).is_err());
    assert!(wallet_price_source_decision(
        |name| match name {
            "ELASTOS_WALLET_PRICE_SOURCE" => Some("coingecko".to_string()),
            _ => None,
        },
        || None,
    )
    .is_err());
    assert!(wallet_price_source_decision(
        |name| match name {
            "ELASTOS_WALLET_PRICE_SOURCE" => Some("coingecko".to_string()),
            "ELASTOS_WALLET_PRICE_HTTP_APPROVED" => Some("1".to_string()),
            _ => None,
        },
        || None,
    )
    .is_ok());
    assert!(wallet_price_source_decision(
        |name| match name {
            "ELASTOS_WALLET_PRICE_SOURCE" => Some("unknown".to_string()),
            "ELASTOS_WALLET_PRICE_HTTP_APPROVED" => Some("1".to_string()),
            _ => None,
        },
        || None,
    )
    .is_err());
    assert!(wallet_price_source_decision(
        |_| None,
        || Some(WalletPricePolicy {
            schema: WALLET_PRICE_POLICY_SCHEMA.to_string(),
            source: "coingecko".to_string(),
            external_http_approved: true,
            approved_by_principal_id: "person:local:admin".to_string(),
            approved_at: 42,
        }),
    )
    .is_ok());
    assert!(wallet_price_source_decision(
        |name| match name {
            "ELASTOS_WALLET_PRICE_SOURCE" => Some("coingecko".to_string()),
            _ => None,
        },
        || Some(WalletPricePolicy {
            schema: WALLET_PRICE_POLICY_SCHEMA.to_string(),
            source: "coingecko".to_string(),
            external_http_approved: false,
            approved_by_principal_id: "person:local:admin".to_string(),
            approved_at: 42,
        }),
    )
    .is_err());
}

#[test]
fn test_wallet_price_source_policy_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    store_wallet_price_http_policy(dir.path(), "person:local:admin", 42).unwrap();
    let policy = load_wallet_price_policy(dir.path()).unwrap();
    assert_eq!(policy.schema, WALLET_PRICE_POLICY_SCHEMA);
    assert_eq!(policy.source, "coingecko");
    assert!(policy.external_http_approved);
    assert_eq!(policy.approved_by_principal_id, "person:local:admin");
}

#[test]
fn test_wallet_price_fetch_audit_records_external_effect() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    append_wallet_price_fetch_audit(
        dir.path(),
        &context,
        "wallet-prices:fetch:42",
        "requested",
        "Wallet requested approved external HTTP market-price fetch",
    )
    .unwrap();
    append_wallet_price_fetch_audit(
        dir.path(),
        &context,
        "wallet-prices:fetch:42",
        "completed",
        "Wallet completed approved external HTTP market-price fetch",
    )
    .unwrap();

    let auth_state = crate::auth::load_auth_state(dir.path()).unwrap();
    let requested = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.price_source.fetch.requested"
                && event.challenge_id.as_deref() == Some("wallet-prices:fetch:42")
        })
        .expect("wallet price fetch requested audit event");
    let event = auth_state
        .audit
        .iter()
        .find(|event| {
            event.event_type == "wallet.price_source.fetch.completed"
                && event.challenge_id.as_deref() == Some("wallet-prices:fetch:42")
        })
        .expect("wallet price fetch audit event");
    assert_ne!(requested.event_id, event.event_id);
    assert_eq!(event.result, "completed");
    assert_eq!(
        event.principal_id.as_deref(),
        Some(context.principal_id.as_str())
    );
    assert_eq!(
        event.session_id.as_deref(),
        Some(context.session_id.as_str())
    );
    assert_eq!(event.capsule_id.as_deref(), Some(WALLET_CAPSULE_ID));
    assert!(!event.signature.as_deref().unwrap_or_default().is_empty());
}

#[test]
fn test_wallet_price_unavailable_response_is_not_an_http_error_shape() {
    let response = wallet_prices_unavailable_response(
        1_731_000_000,
        "external wallet price HTTP source is not approved".to_string(),
    );
    assert!(response.stale);
    assert!(response.unavailable);
    assert!(response.prices.is_empty());
    assert_eq!(
        response.note.as_deref(),
        Some("external wallet price HTTP source is not approved")
    );
}

#[test]
fn test_wallet_qr_svg_rejects_invalid_address_and_encodes_valid_address() {
    let svg = wallet_qr_svg("bc1qw4tn3ck5fvg4xx22dm7a8n7n8j7k4qp99f8r2d").unwrap();
    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("<path d=\"M"));
    assert!(wallet_qr_svg("bad address with spaces").is_err());
}
