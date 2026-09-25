//! R47: a source that answers HTTP 429 is retried briefly, then cooled so
//! that corroborated reads ask it last, and a corroborated read stops asking
//! once two sources agree.

use super::*;

const SELLER: &str = "0x0000000000000000000000000000000000000011";
const LEDGER: &str = "0x0000000000000000000000000000000000000022";
const GATEWAY: &str = "0x00000000000000000000000000000000000000aa";
const OPERATIVE: &str = "0x0000000000000000000000000000000000000044";
const FINALIZED_HASH: &str = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn rate_limited_provider(rpc_url: String) -> ChainProvider {
    let mut provider = provider_with_rpc(rpc_url);
    provider.sleep = record_sleep;
    provider
}

fn esc_network(provider: &ChainProvider) -> ChainNetwork {
    provider.evm_network("esc-local").unwrap().clone()
}

fn requests(counter: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> usize {
    counter.load(std::sync::atomic::Ordering::SeqCst)
}

fn chain_id_reply() -> ScriptedReply {
    ScriptedReply::rpc("eth_chainId", json!([]), json!("0x14"))
}

fn error_code_of<T: std::fmt::Debug>(result: Result<T, Response>) -> String {
    match result {
        Err(Response::Error { code, .. }) => code,
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn evm_rpc_retries_a_rate_limited_request_until_it_succeeds() {
    let (url, counter) = spawn_scripted_rpc_server(vec![
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
        chain_id_reply(),
    ]);
    let provider = rate_limited_provider(url.clone());
    let value = provider
        .evm_rpc(&esc_network(&provider), "eth_chainId", json!([]))
        .expect("the third attempt answers");
    assert_eq!(value, json!("0x14"));
    assert_eq!(requests(&counter), 3);
    assert_eq!(
        recorded_sleeps(),
        vec![Duration::from_millis(250), Duration::from_millis(750)]
    );
    // A source that recovered within its retries is not cooled.
    let sources = vec![url.clone(), "http://127.0.0.1:9".to_string()];
    assert_eq!(provider.corroboration_order(&sources), sources);
}

#[test]
fn evm_rpc_honours_a_retry_after_of_at_most_two_seconds() {
    let (url, counter) = spawn_scripted_rpc_server(vec![
        ScriptedReply::RateLimited(Some("1")),
        ScriptedReply::RateLimited(Some("2")),
        chain_id_reply(),
    ]);
    let provider = rate_limited_provider(url);
    provider
        .evm_rpc(&esc_network(&provider), "eth_chainId", json!([]))
        .expect("the third attempt answers");
    assert_eq!(requests(&counter), 3);
    assert_eq!(
        recorded_sleeps(),
        vec![Duration::from_secs(1), Duration::from_secs(2)]
    );
}

#[test]
fn evm_rpc_caps_a_long_retry_after_and_stops_after_two_retries() {
    let (url, counter) = spawn_scripted_rpc_server(vec![
        ScriptedReply::RateLimited(Some("10")),
        ScriptedReply::RateLimited(Some("10")),
        ScriptedReply::RateLimited(Some("10")),
        chain_id_reply(),
    ]);
    let provider = rate_limited_provider(url.clone());
    assert_eq!(
        error_code_of(provider.evm_rpc(&esc_network(&provider), "eth_chainId", json!([]))),
        "upstream_http_error"
    );
    assert_eq!(requests(&counter), 3, "at most two retries after the first");
    assert_eq!(
        recorded_sleeps(),
        vec![Duration::from_millis(250), Duration::from_millis(750)],
        "a Retry-After above two seconds falls back to the bounded schedule"
    );
}

#[test]
fn evm_rpc_does_not_retry_errors_other_than_429() {
    let (url, counter) = spawn_scripted_rpc_server(vec![ScriptedReply::Status(503)]);
    let provider = rate_limited_provider(url.clone());
    assert_eq!(
        error_code_of(provider.evm_rpc(&esc_network(&provider), "eth_chainId", json!([]))),
        "upstream_http_error"
    );
    assert_eq!(requests(&counter), 1);

    let (rejecting, rejecting_counter) = spawn_scripted_rpc_server(vec![ScriptedReply::Rpc(
        "eth_chainId",
        json!([]),
        RpcReply::Error(json!({ "code": -32005, "message": "limit exceeded" })),
    )]);
    let mut network = esc_network(&provider);
    network.rpc_url = rejecting;
    assert_eq!(
        error_code_of(provider.evm_rpc(&network, "eth_chainId", json!([]))),
        "upstream_rpc_error"
    );
    assert_eq!(requests(&rejecting_counter), 1);

    network.rpc_url = "http://127.0.0.1:9".to_string();
    assert_eq!(
        error_code_of(provider.evm_rpc(&network, "eth_chainId", json!([]))),
        "upstream_unreachable"
    );
    assert!(recorded_sleeps().is_empty());
    // None of these errors cools a source.
    let sources = vec![url, network.rpc_url.clone()];
    assert_eq!(provider.corroboration_order(&sources), sources);
}

#[test]
fn evm_rpc_batch_retries_a_rate_limited_batch() {
    let (url, counter) = spawn_scripted_rpc_server(vec![
        ScriptedReply::RateLimited(None),
        ScriptedReply::Json(json!([{ "jsonrpc": "2.0", "id": 0, "result": "0x1" }])),
    ]);
    let provider = rate_limited_provider(url);
    let answers = provider
        .evm_rpc_batch(
            &esc_network(&provider),
            &[("eth_call".to_string(), json!([]))],
        )
        .expect("the retried batch answers");
    assert_eq!(answers, vec![Some(json!("0x1"))]);
    assert_eq!(requests(&counter), 2);
    assert_eq!(recorded_sleeps(), vec![Duration::from_millis(250)]);
}

#[test]
fn evm_rpc_batch_cools_an_origin_that_stays_rate_limited() {
    let (url, counter) = spawn_scripted_rpc_server(vec![
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
    ]);
    let provider = rate_limited_provider(url.clone());
    assert_eq!(
        error_code_of(provider.evm_rpc_batch(
            &esc_network(&provider),
            &[("eth_call".to_string(), json!([]))],
        )),
        "upstream_http_error"
    );
    assert_eq!(requests(&counter), 3);
    assert_eq!(
        recorded_sleeps(),
        vec![Duration::from_millis(250), Duration::from_millis(750)]
    );
    let other = "http://127.0.0.1:9".to_string();
    assert_eq!(
        provider.corroboration_order(&[url.clone(), other.clone()]),
        vec![other, url]
    );
}

#[test]
fn rate_limit_logs_name_the_host_only() {
    let url = "https://user:secret@rpc.example.com:8443/v2/SECRETKEY?apikey=abc123";
    for line in [
        rate_limit_retry_log(url, 1, Duration::from_millis(250)),
        rate_limit_cooldown_log(url),
    ] {
        assert!(line.contains("rpc.example.com"), "{line}");
        for leaked in ["SECRETKEY", "apikey", "abc123", "secret", "/v2", "user"] {
            assert!(!line.contains(leaked), "log leaked {leaked}: {line}");
        }
    }
    assert!(rate_limit_retry_log(url, 2, Duration::from_millis(750)).contains("retry=2"));
    assert!(rate_limit_retry_log(url, 2, Duration::from_millis(750)).contains("wait_ms=750"));
}

fn at_finalized() -> Value {
    json!({ "blockHash": FINALIZED_HASH, "requireCanonical": true })
}

/// One source's full answer to a verified-listing read (native pay token, so
/// no payment-processor call).
fn verified_listing_script() -> Vec<ScriptedReply> {
    vec![
        chain_id_reply(),
        ScriptedReply::rpc(
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2c", FINALIZED_HASH),
        ),
        ScriptedReply::rpc(
            "eth_call",
            json!([
                { "to": GATEWAY, "data": encode_authority_gateway_operative_call(LEDGER, "0x03").unwrap() },
                at_finalized()
            ]),
            json!(format!("0x{:0>64}", OPERATIVE.trim_start_matches("0x"))),
        ),
        ScriptedReply::rpc(
            "eth_call",
            json!([
                { "to": GATEWAY, "data": encode_authority_gateway_listing_call(OPERATIVE, SELLER).unwrap() },
                at_finalized()
            ]),
            json!(concat!(
                "0x",
                "0000000000000000000000000000000000000000000000000000000000000007",
                "0000000000000000000000000000000000000000000000000000000000000005",
                "0000000000000000000000000000000000000000000000000000000000000000"
            )),
        ),
    ]
}

fn twice(script: Vec<ScriptedReply>) -> Vec<ScriptedReply> {
    let mut both = script.clone();
    both.extend(script);
    both
}

fn market_provider(evidence_rpc_urls: Vec<String>) -> ChainProvider {
    let mut provider = provider_with_rights_rpc_policies_and_purchase(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        json!([]),
        protected_content_market_source(evidence_rpc_urls),
    );
    provider.sleep = record_sleep;
    provider
}

fn resolve_verified_listing(provider: &mut ChainProvider) -> Response {
    provider.handle(Request::ResolveProtectedContentVerifiedListing {
        network: "esc-local".to_string(),
        seller: SELLER.to_string(),
        ledger: LEDGER.to_string(),
        token_id: "0x03".to_string(),
    })
}

#[test]
fn a_persistently_rate_limited_source_is_cooled_and_the_read_corroborates_elsewhere() {
    let (limited, limited_counter) = spawn_scripted_rpc_server(vec![
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
    ]);
    let (second, _) = spawn_scripted_rpc_server(twice(verified_listing_script()));
    let (third, _) = spawn_scripted_rpc_server(twice(verified_listing_script()));
    let sources = vec![limited.clone(), second.clone(), third.clone()];
    let mut provider = market_provider(sources.clone());

    let listing = ok_data(resolve_verified_listing(&mut provider));
    assert_eq!(listing["operative"], OPERATIVE);
    assert_eq!(requests(&limited_counter), 3);
    assert_eq!(
        recorded_sleeps(),
        vec![Duration::from_millis(250), Duration::from_millis(750)]
    );
    assert_eq!(
        provider.corroboration_order(&sources),
        vec![second.clone(), third.clone(), limited.clone()],
        "the cooled origin is asked last"
    );

    // The next read asks the cooled origin last; two others agree first, so
    // it is not asked at all.
    ok_data(resolve_verified_listing(&mut provider));
    assert_eq!(requests(&limited_counter), 3);

    // Sixty seconds on, the cooldown is over and the configured order returns.
    provider.now_unix_seconds = || RIGHTS_EVIDENCE_NOW + 60;
    assert_eq!(provider.corroboration_order(&sources), sources);
}

#[test]
fn a_cooled_source_still_answers_when_nothing_else_can() {
    // The first source is cooled by one persistently rate-limited request,
    // then serves the listing; the only other working source is behind a
    // dead one.
    let mut limited_script = vec![
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
        ScriptedReply::RateLimited(None),
    ];
    limited_script.extend(verified_listing_script());
    let (limited, _) = spawn_scripted_rpc_server(limited_script);
    let (dead, _) = spawn_scripted_rpc_server(Vec::new());
    let (working, _) = spawn_scripted_rpc_server(verified_listing_script());
    let mut provider = market_provider(vec![limited.clone(), dead.clone(), working.clone()]);
    let mut network = esc_network(&provider);
    network.rpc_url = limited.clone();
    assert!(provider
        .evm_rpc(&network, "eth_chainId", json!([]))
        .is_err());
    assert_eq!(
        provider.corroboration_order(&[limited.clone(), dead.clone(), working.clone()]),
        vec![dead, working, limited]
    );

    let listing = ok_data(resolve_verified_listing(&mut provider));
    assert_eq!(listing["operative"], OPERATIVE);
}

#[test]
fn verified_listing_stops_once_two_sources_agree() {
    let (first, _) = spawn_scripted_rpc_server(verified_listing_script());
    let (second, _) = spawn_scripted_rpc_server(verified_listing_script());
    let (third, third_counter) = spawn_scripted_rpc_server(Vec::new());
    let mut provider = market_provider(vec![first, second, third]);
    ok_data(resolve_verified_listing(&mut provider));
    assert_eq!(requests(&third_counter), 0);
}

fn item_script() -> Vec<ScriptedReply> {
    let token_uri =
        "ipfs://bafyfolder/0000000000000000000000000000000000000000000000000000000000000001.json";
    vec![
        chain_id_reply(),
        ScriptedReply::rpc(
            "eth_getBlockByNumber",
            json!(["finalized", false]),
            finalized_block_json("0x2c", FINALIZED_HASH),
        ),
        ScriptedReply::rpc(
            "eth_call",
            json!([
                { "to": GATEWAY, "data": encode_authority_gateway_operative_call(LEDGER, "0x3").unwrap() },
                at_finalized()
            ]),
            json!(format!("0x{:0>64}", OPERATIVE.trim_start_matches("0x"))),
        ),
        ScriptedReply::rpc(
            "eth_call",
            json!([
                { "to": OPERATIVE, "data": encode_operative_token_uri_call().unwrap() },
                at_finalized()
            ]),
            abi_encoded_string_for_test(token_uri),
        ),
    ]
}

#[test]
fn market_item_read_stops_once_two_sources_agree() {
    let (first, _) = spawn_scripted_rpc_server(item_script());
    let (second, _) = spawn_scripted_rpc_server(item_script());
    let (third, third_counter) = spawn_scripted_rpc_server(Vec::new());
    let mut provider = market_provider(vec![first, second, third]);
    let item = ok_data(provider.handle(Request::ResolveProtectedContentItem {
        network: "esc-local".to_string(),
        ledger: LEDGER.to_string(),
        token_id: "0x3".to_string(),
    }));
    assert_eq!(item["operative"], OPERATIVE);
    assert_eq!(requests(&third_counter), 0);
}

#[test]
fn market_kid_binding_read_stops_once_two_sources_agree() {
    let store = "0x00000000000000000000000000000000000000cc";
    let kid = "0x0123456789abcdef0123456789abcdef";
    let script = || {
        vec![
            chain_id_reply(),
            ScriptedReply::rpc(
                "eth_getBlockByNumber",
                json!(["finalized", false]),
                finalized_block_json("0x2c", FINALIZED_HASH),
            ),
            ScriptedReply::rpc(
                "eth_call",
                json!([
                    { "to": GATEWAY, "data": encode_authority_gateway_cstore_call().unwrap() },
                    at_finalized()
                ]),
                json!(format!("0x{:0>64}", store.trim_start_matches("0x"))),
            ),
            ScriptedReply::rpc(
                "eth_call",
                json!([
                    { "to": store, "data": encode_ip_reference_call(kid).unwrap() },
                    at_finalized()
                ]),
                json!(format!(
                    "0x{:0>64}{:0>64}",
                    LEDGER.trim_start_matches("0x"),
                    "3"
                )),
            ),
        ]
    };
    let (first, _) = spawn_scripted_rpc_server(script());
    let (second, _) = spawn_scripted_rpc_server(script());
    let (third, third_counter) = spawn_scripted_rpc_server(Vec::new());
    let mut provider = market_provider(vec![first, second, third]);
    let binding = ok_data(provider.handle(Request::ResolveProtectedContentKidBinding {
        network: "esc-local".to_string(),
        content_access_id: kid.to_string(),
    }));
    assert_eq!(binding["ledger"], LEDGER);
    assert_eq!(requests(&third_counter), 0);
}

#[test]
fn rights_read_stops_once_two_sources_agree() {
    let operation = protected_content_signed_operation();
    let policy = operation.statement().policy_body();
    let expected_data = encode_has_access_by_content_id_call(
        "0x12345678",
        policy.content_access_id().as_bytes(),
        &wallet_subject_hex(&operation),
    )
    .unwrap();
    let script = || {
        vec![
            chain_id_reply(),
            ScriptedReply::rpc(
                "eth_getBlockByNumber",
                json!(["finalized", false]),
                finalized_block_json("0x2a", FINALIZED_HASH),
            ),
            ScriptedReply::rpc(
                "eth_call",
                json!([
                    { "to": "0x0000000000000000000000000000000000000001", "data": expected_data.clone() },
                    at_finalized()
                ]),
                evm_bool_word(true),
            ),
        ]
    };
    let (first, _) = spawn_scripted_rpc_server(script());
    let (second, _) = spawn_scripted_rpc_server(script());
    let (third, third_counter) = spawn_scripted_rpc_server(Vec::new());
    let mut provider = provider_with_rights_rpc_and_policies(
        "http://127.0.0.1:9".to_string(),
        "0x12345678",
        protected_content_policy_sources("view", vec![first, second, third]),
    );
    provider.sleep = record_sleep;
    let evidence = ok_data(provider.handle(Request::ProtectedContentRightsEvidence {
        signed_runtime_release_operation: contract_hex(&operation),
    }));
    assert_eq!(evidence["finalized_block_hash"], json!(FINALIZED_HASH));
    assert_eq!(requests(&third_counter), 0);
}

#[test]
fn market_item_offers_read_stops_once_two_sources_agree() {
    let script = || {
        vec![
            chain_id_reply(),
            ScriptedReply::rpc(
                "eth_getBlockByNumber",
                json!(["finalized", false]),
                finalized_block_json("0x2c", FINALIZED_HASH),
            ),
            ScriptedReply::rpc(
                "eth_call",
                json!([
                    { "to": GATEWAY, "data": encode_authority_gateway_operative_call(LEDGER, "0x3").unwrap() },
                    at_finalized()
                ]),
                json!(format!("0x{:0>64}", OPERATIVE.trim_start_matches("0x"))),
            ),
            ScriptedReply::rpc(
                "eth_call",
                json!([
                    { "to": GATEWAY, "data": encode_authority_gateway_sellers_of_call(OPERATIVE).unwrap() },
                    at_finalized()
                ]),
                abi_encoded_address_array_for_test(&[SELLER]),
            ),
            ScriptedReply::rpc(
                "eth_call",
                json!([
                    { "to": GATEWAY, "data": encode_authority_gateway_listing_call(OPERATIVE, SELLER).unwrap() },
                    at_finalized()
                ]),
                json!(concat!(
                    "0x",
                    "0000000000000000000000000000000000000000000000000000000000000007",
                    "0000000000000000000000000000000000000000000000000000000000000005",
                    "0000000000000000000000000000000000000000000000000000000000000000"
                )),
            ),
        ]
    };
    let (first, _) = spawn_scripted_rpc_server(script());
    let (second, _) = spawn_scripted_rpc_server(script());
    let (third, third_counter) = spawn_scripted_rpc_server(Vec::new());
    let mut provider = market_provider(vec![first, second, third]);
    let offers = ok_data(provider.handle(Request::ResolveProtectedContentItemOffers {
        network: "esc-local".to_string(),
        ledger: LEDGER.to_string(),
        token_id: "0x3".to_string(),
    }));
    assert_eq!(offers["offers"].as_array().unwrap().len(), 1);
    assert_eq!(requests(&third_counter), 0);
}
