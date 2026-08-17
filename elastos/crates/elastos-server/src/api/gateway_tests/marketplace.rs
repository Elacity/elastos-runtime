use super::*;

#[tokio::test]
async fn test_marketplace_catalog_route_is_registered_and_auth_gated() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/catalog")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let token = issue_home_launch_token(dir.path(), MARKETPLACE_CAPSULE_ID).unwrap();
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/catalog")
                .header("x-elastos-home-token", token.clone())
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
    assert_eq!(payload["schema"], "elastos.capsules.catalog/v1");
    assert!(
        payload["capsules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|capsule| capsule["name"] != "bus-v1-conformance"
                && capsule["name"] != "marketplace")
    );

    let marketplace_scoped_response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/marketplace/catalog")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(marketplace_scoped_response.status(), StatusCode::OK);
}

/// The money-verb request body the step-up token binds to (asset + terms the buyer confirmed).
fn buy_intent() -> Value {
    json!({
        "content_id": "0f0e0d0c0b0a09080706050403020100",
        "quantity": "1",
        "expected_price": "1000000000000000000",
    })
}

fn buy_request(home_token: &str, body: &Value) -> Request<Body> {
    test_browser_request("localhost:61180", "http://localhost:61180")
        .method("POST")
        .uri("/api/market/buy")
        .header("x-elastos-home-token", home_token)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_text(response: axum::response::Response) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// `/api/market/buy` is a NODE-SIGNED money verb (buy_authority signs with a managed key and
/// broadcasts). A standing Home session — authenticated, but with no recent passkey ceremony —
/// must NOT be able to spend. Same guard main's wallet/recovery money routes use.
#[tokio::test]
async fn test_market_buy_requires_fresh_passkey_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    let body = buy_intent();
    let denied = app
        .clone()
        .oneshot(buy_request(&authority.home_token, &body))
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "buy without a passkey step-up must be refused"
    );
    let text = response_text(denied).await;
    assert!(
        text.contains("fresh passkey"),
        "refusal must name the step-up requirement, got: {text}"
    );
}

/// The positive path, mirroring `test_create_mint_requires_fresh_passkey_step_up`'s second half:
/// a FRESH, correctly-bound step-up gets `/api/market/buy` PAST the gate. Without this, every buy
/// assertion in this file is satisfied by a route that refuses unconditionally — the gate would
/// look correct while having bricked the verb.
#[tokio::test]
async fn test_market_buy_accepts_a_fresh_bound_passkey_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    let confirmed = buy_intent();
    let token = step_up_token_for_app_context(
        dir.path(),
        HOME_CAPSULE_ID,
        &authority.home_token,
        "market.buy",
        &confirmed,
    );
    let mut approved = confirmed.clone();
    approved["step_up_token"] = json!(token);

    let allowed = app
        .clone()
        .oneshot(buy_request(&authority.home_token, &approved))
        .await
        .unwrap();
    // "Past the gate" is about WHICH check answers, not the status code: on a node with no linked
    // EVM wallet the buy still stops at the wallet-link check, which is also a 403.
    //
    // Assert against the money-verb gate's THREE refusal constants, not a substring. A
    // `!contains("passkey step-up")` check is satisfied by MONEY_VERB_NOT_SIGNED_IN too, so a
    // regression that broke the launch check — and refused every buy as "not signed in" — would
    // have passed. The gate must not answer at all.
    let allowed_text = response_text(allowed).await;
    for refusal in [
        crate::api::viewer_open::MONEY_VERB_NOT_SIGNED_IN,
        crate::api::viewer_open::MONEY_VERB_STEP_UP_REQUIRED,
        crate::api::viewer_open::MONEY_VERB_STEP_UP_REJECTED,
    ] {
        assert_ne!(
            allowed_text.trim(),
            refusal,
            "a fresh, correctly-bound step-up must pass the money-verb gate"
        );
    }
    // Positively: the next check down the line is the one that answered. Which check that is
    // depends on the rights lane — in RightsMode::Chain (the default) the buy stops at the
    // wallet-link check; in RightsMode::Dev (`--features dev-modes`) there is no wallet to link
    // and the buy completes outright. Either shape proves the gate admitted the token.
    let completed_buy = serde_json::from_str::<serde_json::Value>(&allowed_text)
        .map(|payload| payload["owned_now"] == json!(true))
        .unwrap_or(false);
    assert!(
        allowed_text.contains("link an EVM wallet") || completed_buy,
        "expected the buy to reach the wallet-link check or complete with owned_now, got: {allowed_text}"
    );

    // Single-use: the SAME token cannot drive a second spend of the same intent.
    let replayed = app
        .clone()
        .oneshot(buy_request(&authority.home_token, &approved))
        .await
        .unwrap();
    assert_eq!(
        replayed.status(),
        StatusCode::FORBIDDEN,
        "a consumed step-up must not authorize a second buy"
    );
}

/// A money-verb refusal must not narrate the node's internals. Whatever the gate failed on —
/// token decode, envelope schema, replay-state IO, a filesystem path under the data dir — the
/// caller sees one of three fixed reasons, chosen by what they can DO about it.
#[tokio::test]
async fn test_market_buy_refusals_expose_no_internal_detail() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    let mut malformed = buy_intent();
    malformed["step_up_token"] = json!("not-a-real-token");

    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "no step-up at all",
            buy_intent(),
            Some(authority.home_token.clone()),
        ),
        (
            "undecodable step-up token",
            malformed,
            Some(authority.home_token.clone()),
        ),
        ("no Home launch at all", buy_intent(), None),
    ];

    for (label, body, home_token) in cases {
        let request = match &home_token {
            Some(token) => buy_request(token, &body),
            None => test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/market/buy")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        };
        let denied = app.clone().oneshot(request).await.unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN, "{label}");
        let text = response_text(denied).await;
        for leak in [
            dir.path().to_string_lossy().as_ref(),
            "step-up state",
            "consumed",
            "envelope",
            "schema",
            "serde",
            "os error",
            "No such file",
            ".json",
        ] {
            assert!(
                !text.contains(leak),
                "{label}: refusal leaked internal detail ({leak}): {text}"
            );
        }
        assert!(
            text.len() < 256,
            "{label}: refusal is a reason, not a dump: {text}"
        );
    }
}

/// Freshness, not mere presence: a step-up assertion older than the money-verb window is refused.
#[tokio::test]
async fn test_market_buy_rejects_stale_passkey_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    let body = buy_intent();
    let stale = stale_step_up_token_for_app_context(
        dir.path(),
        HOME_CAPSULE_ID,
        &authority.home_token,
        "market.buy",
        &body,
    );
    let mut with_stale = body.clone();
    with_stale["step_up_token"] = json!(stale);

    let denied = app
        .clone()
        .oneshot(buy_request(&authority.home_token, &with_stale))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let text = response_text(denied).await;
    assert!(
        text.contains("passkey step-up") || text.contains("fresh passkey"),
        "stale step-up refusal must name the step-up, got: {text}"
    );
}

/// Intent binding: a step-up minted for ONE purchase cannot be replayed onto a different one
/// (different asset / different agreed price).
#[tokio::test]
async fn test_market_buy_rejects_step_up_bound_to_another_purchase() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    let confirmed = buy_intent();
    let token = step_up_token_for_app_context(
        dir.path(),
        HOME_CAPSULE_ID,
        &authority.home_token,
        "market.buy",
        &confirmed,
    );

    let mut swapped = confirmed.clone();
    swapped["expected_price"] = json!("9000000000000000000");
    swapped["step_up_token"] = json!(token);

    let denied = app
        .clone()
        .oneshot(buy_request(&authority.home_token, &swapped))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let text = response_text(denied).await;
    assert!(
        text.contains("passkey step-up"),
        "intent-mismatch refusal must name the step-up, got: {text}"
    );
}

/// `/api/create/mint` is the sibling node-signed money verb (mint_authority signs + broadcasts).
/// Same gate; and a FRESH, correctly-bound step-up lets the request through to the mint itself.
#[tokio::test]
async fn test_create_mint_requires_fresh_passkey_step_up() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    let assembly = json!({
        "to": "0x1111111111111111111111111111111111111111",
        "token_uri": "ipfs://bafy-test",
        "op_type_code": 1,
        "content_id": "0f0e0d0c0b0a09080706050403020100",
    });
    let mint_request = |body: &Value| {
        test_browser_request("localhost:61180", "http://localhost:61180")
            .method("POST")
            .uri("/api/create/mint")
            .header("x-elastos-home-token", authority.home_token.as_str())
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };

    let denied = app.clone().oneshot(mint_request(&assembly)).await.unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::FORBIDDEN,
        "mint without a passkey step-up must be refused"
    );
    let text = response_text(denied).await;
    assert!(
        text.contains("fresh passkey"),
        "refusal must name the step-up requirement, got: {text}"
    );

    let token = step_up_token_for_app_context(
        dir.path(),
        HOME_CAPSULE_ID,
        &authority.home_token,
        "create.mint",
        &assembly,
    );
    let mut approved = assembly.clone();
    approved["step_up_token"] = json!(token);
    let allowed = app.clone().oneshot(mint_request(&approved)).await.unwrap();
    assert_ne!(
        allowed.status(),
        StatusCode::FORBIDDEN,
        "a fresh, correctly-bound step-up must pass the gate"
    );
}
