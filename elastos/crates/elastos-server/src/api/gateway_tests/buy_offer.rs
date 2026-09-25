//! `buy_offer`: buying one copy of any live offer on the terms the buyer
//! agreed to (§5.2, §5.3), against the same chain and content stubs as the
//! `ListingObject` tests in `listing_object`.

use super::listing_object::*;
use super::*;

/// What the page sends (§5.2, R20): the item and asset it was shown, the
/// seller it chose, and the terms it agreed to. No operative -- Runtime
/// derives it -- and the seller only once, at the top level.
fn buy_offer_body(seller: &str, price: &str, pay_token: &str) -> Value {
    json!({
        "item": {
            "chain_namespace": "eip155:8453",
            "network": "base-mainnet",
            "ledger": LISTING_LEDGER,
            "token_id": LISTING_TOKEN_ID,
            "kid": LISTING_KID,
        },
        "asset_uri": format!("elastos://{LISTING_FOLDER}"),
        "seller": seller,
        "agreed": {
            "price": price,
            "pay_token": pay_token,
            "quantity": "0x1",
        },
    })
}

fn native_buy_body() -> Value {
    buy_offer_body(
        LISTING_NATIVE_SELLER,
        LISTING_NATIVE_PRICE,
        LISTING_NATIVE_PAY_TOKEN,
    )
}

async fn buy_env() -> ListingEnv {
    ListingEnv::new(
        chain_with_item(LISTING_FOLDER, LISTING_KID),
        content_with(
            LISTING_FOLDER,
            "metadata.json",
            &elastos_v1_metadata(LISTING_KID),
        ),
    )
    .await
}

fn assert_refused(answer: &Value, code: &str) {
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(answer["code"], code, "{answer}");
}

/// Review focus 1: the seller moved the terms between view and buy. The
/// buyer is told the new terms and nothing reaches the Wallet.
#[tokio::test]
async fn buy_offer_answers_terms_changed_without_raising_an_effect() {
    let env = buy_env().await;

    // A price the seller no longer asks.
    let (status, answer) = env
        .buy_offer(buy_offer_body(
            LISTING_NATIVE_SELLER,
            "0x1",
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_refused(&answer, "terms_changed");
    assert_eq!(
        answer["current"],
        json!({
            "seller": LISTING_NATIVE_SELLER,
            "quantity": "0x1",
            "price": LISTING_NATIVE_PRICE,
            "pay_token": LISTING_NATIVE_PAY_TOKEN,
            "payment_processor": null,
        })
    );

    // A pay token the offer is not priced in.
    let (_, answer) = env
        .buy_offer(buy_offer_body(
            LISTING_SELLER,
            "0xf4240",
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_refused(&answer, "terms_changed");
    assert_eq!(answer["current"]["pay_token"], LISTING_PAY_TOKEN);
    assert_eq!(
        answer["current"]["payment_processor"],
        LISTING_PAYMENT_PROCESSOR
    );

    // A seller with no offer at all.
    let (_, answer) = env
        .buy_offer(buy_offer_body(
            LISTING_OTHER_OPERATIVE,
            LISTING_NATIVE_PRICE,
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_refused(&answer, "terms_changed");
    assert_eq!(answer["current"], Value::Null);

    // An offer that has sold out.
    env.chain.lock().unwrap().offers = Some(json!([{
        "seller": LISTING_NATIVE_SELLER,
        "quantity": "0x0",
        "price": LISTING_NATIVE_PRICE,
        "pay_token": LISTING_NATIVE_PAY_TOKEN,
    }]));
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_refused(&answer, "terms_changed");
    assert_eq!(answer["current"], Value::Null);

    assert_eq!(
        env.wallet_transaction_requests().await,
        0,
        "changed terms need a new decision; nothing may reach the Wallet"
    );
    assert!(env.market_purchase().is_none());
}

/// Review focus 4: a reload or a second press resumes the same effect on the
/// terms recorded when it started, whatever the new request says.
#[tokio::test]
async fn buy_offer_resumes_the_recorded_attempt() {
    let env = buy_env().await;
    let (status, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["status"], "error", "{first}");
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    assert_eq!(first["buy_progress"]["resumable"], true);
    let recorded = env.market_purchase().expect("recorded before the effect");
    assert_eq!(env.wallet_transaction_requests().await, 1);
    assert_eq!(
        env.wallet.latest_transaction_approval_request_id().await,
        Some(recorded.acquisition_stage.approval_request_id.clone())
    );

    // The same press again, on the same terms: the recorded attempt resumes.
    let (_, second) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        second["buy_progress"]["stage"], "purchase_approval",
        "{second}"
    );
    assert_eq!(env.wallet_transaction_requests().await, 1);
    let resumed = env.market_purchase().unwrap();
    assert_eq!(
        resumed.acquisition_stage.effect_id,
        recorded.acquisition_stage.effect_id
    );
    assert_eq!(resumed.offer, recorded.offer);
    assert_eq!(resumed.offer.price, LISTING_NATIVE_PRICE);
    assert_eq!(resumed.attempt_id, recorded.attempt_id);
    assert_eq!(resumed.attempt_id.len(), 32);
    assert!(resumed
        .attempt_id
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}

#[tokio::test]
async fn buy_offer_refuses_the_buyers_own_offer() {
    let env = buy_env().await;
    let own = env.buyer_address.to_ascii_lowercase();
    env.chain.lock().unwrap().offers = Some(json!([{
        "seller": own,
        "quantity": "0x1",
        "price": LISTING_NATIVE_PRICE,
        "pay_token": LISTING_NATIVE_PAY_TOKEN,
    }]));
    let (status, answer) = env
        .buy_offer(buy_offer_body(
            &own,
            LISTING_NATIVE_PRICE,
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_refused(&answer, "own_offer");
    assert!(answer.get("current").is_none());
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

/// This person already holds the item -- here, bought through the
/// listing-package path -- so a press never pays for it again. R46: the buy
/// knows no KID, so it asks the chain no access question; the records this
/// Home keeps answer it.
#[tokio::test]
async fn buy_offer_refuses_an_item_this_person_already_owns() {
    let env = buy_env().await;
    crate::protected_content_runtime::persist_runtime_custody_purchase(
        env._dir.path(),
        &completed_listing_purchase(&env.authority.principal_id, &env.buyer_address),
    )
    .unwrap();
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_refused(&answer, "already_owned");
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
    assert!(chain_ops(&env, "resolve_protected_content_purchase_access").is_empty());
}

/// D1: a buy runs no read-side check. The content plane answers nothing but
/// the shared document the binding check reads, and the purchase still
/// completes.
#[tokio::test]
async fn buy_offer_completes_without_an_availability_receipt() {
    let env = buy_env().await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;

    let (status, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "ok", "{answer}");
    let complete = &answer["data"];
    assert_eq!(
        *complete,
        json!({
            "schema": "elastos.marketplace.buy-offer-complete/v1",
            "item": {
                "chain_namespace": "eip155:8453",
                "network": "base-mainnet",
                "ledger": LISTING_LEDGER,
                "token_id": LISTING_TOKEN_ID,
                "operative": LISTING_OPERATIVE,
                "kid": LISTING_KID,
            },
            "asset_uri": format!("elastos://{LISTING_FOLDER}"),
            "adoption": "pending",
        })
    );
    let record = env.market_purchase().unwrap();
    assert!(matches!(
        record.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ));
    // Access evidence was asked with the verified KID.
    let access_kids = env
        .chain
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| request["op"] == "resolve_protected_content_purchase_access")
        .map(|request| request["content_access_id"].clone())
        .collect::<Vec<_>>();
    assert!(!access_kids.is_empty());
    assert!(access_kids.iter().all(|kid| *kid == LISTING_KID));

    // Asking again is the same answer, and nothing new is raised.
    let (_, again) = env.buy_offer(native_buy_body()).await;
    assert_eq!(again["data"], *complete);
    assert_eq!(env.wallet_transaction_requests().await, 1);

    // Nothing but the shared document was ever read: no availability
    // `ensure`, no `manifest.json`, no content.
    let paths = env.content_paths();
    assert!(
        paths.iter().all(|path| path == "metadata.json"),
        "a buy reads only the shared document: {paths:?}"
    );
    let bare = env
        .buyer_address
        .trim_start_matches("0x")
        .to_ascii_lowercase();
    assert!(!serde_json::to_string(complete)
        .unwrap()
        .to_ascii_lowercase()
        .contains(&bare));
}

/// The request's asset URI and chain are claims; each is re-derived from the
/// chain and a difference is `asset_mismatch`, before anything is raised.
/// R46: its KID is not one of them.
#[tokio::test]
async fn buy_offer_refuses_an_item_claim_that_does_not_bind() {
    let env = buy_env().await;
    let wrong = |pointer: &str, value: Value| {
        let mut body = native_buy_body();
        *body.pointer_mut(pointer).unwrap() = value;
        body
    };
    for body in [
        wrong(
            "/asset_uri",
            json!(format!("elastos://{LISTING_COPY_FOLDER}")),
        ),
        wrong("/item/network", json!("esc-mainnet")),
    ] {
        let (status, answer) = env.buy_offer(body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_refused(&answer, "asset_mismatch");
        assert!(answer.get("current").is_none());
    }
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());

    // An item the chain names no operative for is no item on this market.
    let mut chain = chain_with_item(LISTING_FOLDER, LISTING_KID);
    chain.items.insert(
        (LISTING_LEDGER.to_string(), LISTING_TOKEN_ID.to_string()),
        (
            LISTING_NATIVE_PAY_TOKEN.to_string(),
            listing_token_uri(LISTING_FOLDER),
        ),
    );
    let env = ListingEnv::new(chain, ListingContentFixture::default()).await;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(
        answer["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_UNBOUND_MESSAGE
    );
    assert_eq!(env.wallet_transaction_requests().await, 0);
}

#[tokio::test]
async fn buy_offer_is_refused_to_every_capsule_but_marketplace() {
    let env = buy_env().await;
    for capsule in [LIBRARY_CAPSULE_ID, CREATOR_CAPSULE_ID] {
        let token = app_token_for_authority(env._dir.path(), capsule, &env.authority);
        let (status, answer) = env.buy_offer_with_token(&token, native_buy_body()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{capsule}: {answer}");
    }
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

/// M1: a revert known only from the node's words (the broadcast was refused
/// as "execution reverted") proves nothing is spent, so the attempt is kept
/// and the answer is the plain failure -- never `terms_changed`, which would
/// invite new terms the kept attempt cannot take. The terms are moved here to
/// show that even then no `terms_changed` is answered.
#[tokio::test]
async fn buy_offer_answers_plain_failure_on_a_text_only_revert() {
    let env = buy_env().await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    env.wallet.complete_latest_transaction_approval().await;
    {
        let mut chain = env.chain.lock().unwrap();
        chain.revert_broadcast = true;
        chain.offers = Some(json!([{
            "seller": LISTING_NATIVE_SELLER,
            "quantity": "0x1",
            "price": "0x2386f26fc10001",
            "pay_token": LISTING_NATIVE_PAY_TOKEN,
        }]));
    }
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(answer["code"], "library_error", "{answer}");
    assert_eq!(
        answer["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_UNAVAILABLE_MESSAGE
    );
    assert_eq!(env.wallet_transaction_requests().await, 1);
    // A refused broadcast is not proof the effect is spent -- it may yet be
    // on the chain -- so this attempt is kept, never retired.
    assert!(env.market_purchase().is_some());
}

/// R20: the request the page really builds. This body is byte for byte what
/// `buyOfferRequestBody` in capsules/marketplace/browser/marketplace.js
/// produces (`JSON.stringify`, its key order, no operative, `kid` only when
/// the listing carried one, the seller once, at the top) for the native offer
/// of this item -- and it buys the item end to end.
#[tokio::test]
async fn buy_offer_accepts_the_request_the_page_builds() {
    let page_body = |with_kid: bool| {
        let kid = if with_kid {
            format!(r#","kid":"{LISTING_KID}""#)
        } else {
            String::new()
        };
        format!(
            r#"{{"item":{{"chain_namespace":"eip155:8453","network":"base-mainnet","ledger":"{LISTING_LEDGER}","token_id":"{LISTING_TOKEN_ID}"{kid}}},"asset_uri":"elastos://{LISTING_FOLDER}","seller":"{LISTING_NATIVE_SELLER}","agreed":{{"price":"{LISTING_NATIVE_PRICE}","pay_token":"{LISTING_NATIVE_PAY_TOKEN}","quantity":"0x1"}}}}"#
        )
        .into_bytes()
    };
    for with_kid in [true, false] {
        let env = buy_env().await;
        let token = env.token.clone();
        let (status, first) = env.buy_offer_raw(&token, page_body(with_kid)).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(
            first["buy_progress"]["stage"], "purchase_approval",
            "{first}"
        );
        env.wallet.complete_latest_transaction_approval().await;
        env.chain.lock().unwrap().has_access = true;
        let (_, answer) = env.buy_offer_raw(&token, page_body(with_kid)).await;
        assert_eq!(answer["status"], "ok", "{answer}");
        // The operative and, when the page sent none, the KID are the ones
        // the binding check derived.
        assert_eq!(answer["data"]["item"]["operative"], LISTING_OPERATIVE);
        assert_eq!(answer["data"]["item"]["kid"], LISTING_KID);
        assert_eq!(env.wallet_transaction_requests().await, 1);
    }
}

/// The wire is exactly §5.2: a field it does not name is refused before
/// anything is read or raised -- including an `operative` the page might
/// claim.
#[tokio::test]
async fn buy_offer_refuses_a_request_with_an_unknown_field() {
    let env = buy_env().await;
    let mut claims_operative = native_buy_body();
    claims_operative["item"]["operative"] = json!(LISTING_OPERATIVE);
    let mut agreed_seller = native_buy_body();
    agreed_seller["agreed"]["seller"] = json!(LISTING_NATIVE_SELLER);
    let mut extra = native_buy_body();
    extra["mint_id"] = json!("00");
    for body in [claims_operative, agreed_seller, extra] {
        let (status, answer) = env.buy_offer(body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(answer["status"], "error", "{answer}");
        assert!(
            answer["message"]
                .as_str()
                .unwrap_or_default()
                .contains("unknown field"),
            "{answer}"
        );
    }
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

/// R21: a buy mined and reverted is a spent effect. The attempt is retired,
/// so the buyer's next press, on the terms they now agree to, is a fresh
/// attempt with a fresh effect.
#[tokio::test]
async fn buy_offer_retires_an_attempt_whose_buy_reverted() {
    let env = buy_env().await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    let dead = env.market_purchase().unwrap();
    env.wallet.complete_latest_transaction_approval().await;
    const MOVED_PRICE: &str = "0x2386f26fc10001";
    {
        let mut chain = env.chain.lock().unwrap();
        chain.revert_receipt = true;
        chain.offers = Some(json!([{
            "seller": LISTING_NATIVE_SELLER,
            "quantity": "0x1",
            "price": MOVED_PRICE,
            "pay_token": LISTING_NATIVE_PAY_TOKEN,
        }]));
    }
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_refused(&answer, "terms_changed");
    assert_eq!(answer["current"]["price"], MOVED_PRICE);
    assert!(
        env.market_purchase().is_none(),
        "a reverted attempt is dead and must not be resumed"
    );

    env.chain.lock().unwrap().revert_receipt = false;
    let (_, fresh) = env
        .buy_offer(buy_offer_body(
            LISTING_NATIVE_SELLER,
            MOVED_PRICE,
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_eq!(
        fresh["buy_progress"]["stage"], "purchase_approval",
        "{fresh}"
    );
    let attempt = env.market_purchase().unwrap();
    assert_ne!(
        attempt.acquisition_stage.effect_id,
        dead.acquisition_stage.effect_id
    );
    assert_eq!(attempt.offer.price, MOVED_PRICE);
    assert_eq!(env.wallet_transaction_requests().await, 2);

    // The same revert on terms that did not move is today's failure -- and
    // the attempt is retired all the same.
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().revert_receipt = true;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(answer["code"], "library_error", "{answer}");
    assert!(env.market_purchase().is_none());
}

/// R23: a buy that reverted at terms that did not move (the buyer's gas or
/// funds ran out) is retired, and pressing Buy again on the SAME terms is a
/// new attempt with a new effect -- never the spent one -- and it completes.
#[tokio::test]
async fn buy_offer_retries_a_reverted_buy_at_unchanged_terms_as_a_new_attempt() {
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    let dead = env.market_purchase().unwrap();
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().revert_receipt = true;
    let (_, failed) = env.buy_offer(native_buy_body()).await;
    assert_eq!(failed["code"], "library_error", "{failed}");
    assert!(env.market_purchase().is_none());

    env.chain.lock().unwrap().revert_receipt = false;
    let (_, retry) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        retry["buy_progress"]["stage"], "purchase_approval",
        "{retry}"
    );
    let attempt = env.market_purchase().unwrap();
    assert_ne!(
        attempt.acquisition_stage.effect_id, dead.acquisition_stage.effect_id,
        "the same terms again are a new attempt, not the spent effect"
    );
    assert_ne!(attempt.attempt_id, dead.attempt_id);
    assert_eq!(attempt.offer, dead.offer);
    assert_eq!(env.wallet_transaction_requests().await, 2);

    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;
    let (_, done) = env.buy_offer(native_buy_body()).await;
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(
        done["data"]["schema"],
        "elastos.marketplace.buy-offer-complete/v1"
    );
}

/// R21's other half: an attempt still waiting on the Wallet is alive. The
/// chain's terms moving does not retire it -- the contract enforces the
/// recorded terms, so a late approval can never charge different ones. A press
/// on the new terms is told an attempt is in progress (R28); a press on the
/// recorded terms resumes it.
#[tokio::test]
async fn buy_offer_keeps_resuming_a_pending_attempt_when_chain_terms_move() {
    let env = buy_env().await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    let recorded = env.market_purchase().unwrap();
    env.chain.lock().unwrap().offers = Some(json!([{
        "seller": LISTING_NATIVE_SELLER,
        "quantity": "0x1",
        "price": "0x2386f26fc10001",
        "pay_token": LISTING_NATIVE_PAY_TOKEN,
    }]));
    let (_, moved) = env
        .buy_offer(buy_offer_body(
            LISTING_NATIVE_SELLER,
            "0x2386f26fc10001",
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_refused(&moved, "attempt_in_progress");
    assert_eq!(moved["current"]["price"], LISTING_NATIVE_PRICE, "{moved}");
    let (_, again) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        again["buy_progress"]["stage"], "purchase_approval",
        "{again}"
    );
    assert_eq!(env.wallet_transaction_requests().await, 1);
    let mut resumed = env.market_purchase().unwrap();
    resumed.updated_at = recorded.updated_at;
    assert_eq!(resumed, recorded, "the same attempt, on its recorded terms");
}

/// A catalog row as the index states the item, before this Home says
/// anything about it.
fn catalog_row(token_id: &str) -> MarketCatalogItem {
    MarketCatalogItem {
        ledger: LISTING_LEDGER.to_string(),
        token_id: token_id.to_string(),
        operative: LISTING_OPERATIVE.to_string(),
        seller_address: LISTING_NATIVE_SELLER.to_string(),
        content_access_id: LISTING_KID.to_string(),
        content_cid: String::new(),
        metadata_cid: LISTING_FOLDER.to_string(),
        display_name: "Tradingviewww".to_string(),
        content_category: "video".to_string(),
        price: LISTING_NATIVE_PRICE.to_string(),
        pay_token: LISTING_NATIVE_PAY_TOKEN.to_string(),
        quantity: 1,
        op_type: 1,
        views: 0,
        published_at: 1,
        mint_id: String::new(),
        access_state: "available".to_string(),
    }
}

/// Review focus 5: a foreign asset -- no ElastOS protection anywhere in its
/// shared document, as an ela.city (Lit) mint writes it -- is bought all the
/// same (D8). The purchase completes, adoption says `foreign`, nothing
/// read-side is invented for it, and the catalog shows it as owned.
#[tokio::test]
async fn foreign_purchase_completes_without_adoption() {
    let env = ListingEnv::new(
        chain_with_item(LISTING_FOLDER, LISTING_KID),
        content_with(LISTING_FOLDER, "metadata.json", &lit_metadata(LISTING_KID)),
    )
    .await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;
    let (status, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(answer["data"]["adoption"], "foreign", "{answer}");

    let record = env.market_purchase().unwrap();
    assert!(matches!(
        record.progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ));
    assert_eq!(record.adopted_mint_id, None);
    assert!(
        crate::protected_content_runtime::runtime_custody_listing_chain_index(
            env._dir.path(),
            &env.authority.principal_id,
        )
        .unwrap()
        .is_empty(),
        "a foreign asset gets no listing record"
    );
    let paths = env.content_paths();
    assert!(
        paths.iter().all(|path| path == "metadata.json"),
        "only the shared document is read: {paths:?}"
    );

    let mut items = vec![
        catalog_row(LISTING_TOKEN_ID),
        catalog_row(LISTING_OTHER_TOKEN_ID),
    ];
    mark_held_market_catalog_items(
        env._dir.path(),
        &env.authority.principal_id,
        Some("eip155:8453"),
        &mut items,
    );
    assert_eq!(items[0].access_state, "purchased");
    assert_eq!(items[0].mint_id, "", "a foreign asset has no mint here");
    assert_eq!(items[1].access_state, "available");

    // Another principal on the same Home owns nothing.
    let mut theirs = vec![catalog_row(LISTING_TOKEN_ID)];
    mark_held_market_catalog_items(
        env._dir.path(),
        "someone-else",
        Some("eip155:8453"),
        &mut theirs,
    );
    assert_eq!(theirs[0].access_state, "available");

    // T8-M2: the same ledger and token id on another chain is another item.
    let mut elsewhere = vec![catalog_row(LISTING_TOKEN_ID)];
    mark_held_market_catalog_items(
        env._dir.path(),
        &env.authority.principal_id,
        Some("eip155:20"),
        &mut elsewhere,
    );
    assert_eq!(elsewhere[0].access_state, "available");
}

/// The capsule's `parseBuyOfferAnswer` fixture is written by this producer,
/// never by hand: one real answer of each kind `buy_offer` gives, exactly as
/// the page receives it (the provider error envelope for a refusal or a wait,
/// the success payload for a completed purchase).
#[tokio::test]
async fn buy_offer_writes_the_capsule_parser_fixture() {
    let mut answers = serde_json::Map::new();

    let env = buy_env().await;
    let (_, answer) = env
        .buy_offer(buy_offer_body(
            LISTING_NATIVE_SELLER,
            "0x1",
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_refused(&answer, "terms_changed");
    answers.insert("terms_changed_native".into(), answer);
    let (_, answer) = env
        .buy_offer(buy_offer_body(
            LISTING_OTHER_OPERATIVE,
            LISTING_NATIVE_PRICE,
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_refused(&answer, "terms_changed");
    answers.insert("terms_changed_gone".into(), answer);
    let mut body = native_buy_body();
    body["asset_uri"] = json!(format!("elastos://{LISTING_COPY_FOLDER}"));
    let (_, answer) = env.buy_offer(body).await;
    assert_refused(&answer, "asset_mismatch");
    answers.insert("asset_mismatch".into(), answer);

    let (_, answer) = env
        .buy_offer(buy_offer_body(LISTING_SELLER, "0x1", LISTING_PAY_TOKEN))
        .await;
    assert_refused(&answer, "terms_changed");
    assert_eq!(
        answer["current"]["payment_processor"],
        LISTING_PAYMENT_PROCESSOR
    );
    answers.insert("terms_changed_erc20".into(), answer);

    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        answer["buy_progress"]["stage"], "purchase_approval",
        "{answer}"
    );
    answers.insert("wait".into(), answer);
    // R28: another seller pressed while the native attempt waits.
    let (_, answer) = env
        .buy_offer(buy_offer_body(LISTING_SELLER, "0xf4240", LISTING_PAY_TOKEN))
        .await;
    assert_refused(&answer, "attempt_in_progress");
    assert_eq!(answer["current"], recorded_native_offer());
    answers.insert("attempt_in_progress".into(), answer);
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "ok", "{answer}");
    answers.insert("complete".into(), answer["data"].clone());

    let env = buy_env().await;
    crate::protected_content_runtime::persist_runtime_custody_purchase(
        env._dir.path(),
        &completed_listing_purchase(&env.authority.principal_id, &env.buyer_address),
    )
    .unwrap();
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_refused(&answer, "already_owned");
    answers.insert("already_owned".into(), answer);

    let env = buy_env().await;
    let own = env.buyer_address.to_ascii_lowercase();
    env.chain.lock().unwrap().offers = Some(json!([{
        "seller": own,
        "quantity": "0x1",
        "price": LISTING_NATIVE_PRICE,
        "pay_token": LISTING_NATIVE_PAY_TOKEN,
    }]));
    let (_, answer) = env
        .buy_offer(buy_offer_body(
            &own,
            LISTING_NATIVE_PRICE,
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_refused(&answer, "own_offer");
    answers.insert("own_offer".into(), answer);

    // R29: the listing-package path is buying this item.
    let env = buy_env().await;
    crate::protected_content_runtime::persist_runtime_custody_purchase(
        env._dir.path(),
        &pending_listing_purchase(&env.authority.principal_id, &env.buyer_address),
    )
    .unwrap();
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_refused(&answer, "attempt_in_progress");
    assert_eq!(answer["current"], Value::Null);
    answers.insert("attempt_in_progress_other_path".into(), answer);

    // A declined approval: a wait that will not resume.
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    {
        let mut approvals = env.wallet.approvals.lock().await;
        let approval = approvals
            .iter_mut()
            .rev()
            .find(|approval| approval["intent"] == "transaction_intent")
            .unwrap();
        approval["status"] = json!("rejected");
    }
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["buy_progress"]["stage"], "declined", "{answer}");
    assert_eq!(answer["buy_progress"]["resumable"], false, "{answer}");
    answers.insert("declined".into(), answer);

    // A foreign asset, bought.
    let env = ListingEnv::new(
        chain_with_item(LISTING_FOLDER, LISTING_KID),
        content_with(LISTING_FOLDER, "metadata.json", &lit_metadata(LISTING_KID)),
    )
    .await;
    env.buy_offer(native_buy_body()).await;
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["data"]["adoption"], "foreign", "{answer}");
    answers.insert("complete_foreign".into(), answer["data"].clone());

    let serialized = serde_json::to_string(&answers)
        .unwrap()
        .to_ascii_lowercase();
    assert!(
        !serialized.contains(&own.trim_start_matches("0x").to_string()),
        "no answer may carry this Home's account"
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../capsules/marketplace/browser/src/fixtures/buy-offer-answers.json");
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(answers)).unwrap();
    bytes.push(b'\n');
    std::fs::write(&path, bytes).unwrap();
}

/// A newer attempt for the same item, written as another driver would: the
/// recorded one with a different attempt id. Returns its attempt id.
fn replace_with_a_newer_attempt(data_dir: &std::path::Path, principal_id: &str) -> String {
    let mut newer = crate::protected_content_market::load_runtime_market_purchase(
        data_dir,
        principal_id,
        &listing_market_item(),
    )
    .unwrap()
    .expect("a recorded attempt to supersede");
    newer.attempt_id = "fedcba9876543210fedcba9876543210".to_string();
    newer.progress = crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
        confirmed_approval: None,
        confirmed_buy: None,
    };
    crate::protected_content_market::persist_runtime_market_purchase(data_dir, &newer).unwrap();
    newer.attempt_id
}

fn install_newer_attempt_hook(env: &ListingEnv, op: &'static str) {
    let data_dir = env._dir.path().to_path_buf();
    let principal_id = env.authority.principal_id.clone();
    env.chain.lock().unwrap().hooks.insert(
        op,
        Box::new(move || {
            replace_with_a_newer_attempt(&data_dir, &principal_id);
        }),
    );
}

/// R26: two first presses at once -- a double click, two tabs -- are ONE
/// attempt: one record, one Wallet request, one effect. The press that loses
/// the race to record the attempt resumes the one that won.
#[tokio::test]
async fn buy_offer_concurrent_first_presses_raise_one_effect() {
    let env = buy_env().await;
    env.chain.lock().unwrap().slow_purchase_read = true;
    let (first, second) = tokio::join!(
        env.buy_offer(native_buy_body()),
        env.buy_offer(native_buy_body())
    );
    for (_, answer) in [&first, &second] {
        assert_eq!(
            answer["buy_progress"]["stage"], "purchase_approval",
            "{answer}"
        );
    }
    assert_eq!(
        env.wallet_transaction_requests().await,
        1,
        "two presses must never be two payments"
    );
    let recorded = env.market_purchase().unwrap();
    assert_eq!(
        env.wallet.latest_transaction_approval_request_id().await,
        Some(recorded.acquisition_stage.approval_request_id.clone())
    );
    // And the attempt both answered about completes once.
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;
    let (_, done) = env.buy_offer(native_buy_body()).await;
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(env.wallet_transaction_requests().await, 1);
}

/// R26: a slow driver of an older attempt that sees its buy reverted may
/// retire only its OWN attempt -- never a newer one recorded meanwhile.
#[tokio::test]
async fn buy_offer_stale_driver_does_not_retire_a_newer_attempt() {
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().revert_receipt = true;
    // While this driver reads its buy's receipt, a newer attempt takes the
    // record's place.
    install_newer_attempt_hook(&env, "receipt");
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    let kept = env
        .market_purchase()
        .expect("the newer attempt must not be retired by a stale driver");
    assert_eq!(kept.attempt_id, "fedcba9876543210fedcba9876543210");
}

/// R26: a slow driver of an older attempt never overwrites a newer attempt
/// with its own progress.
#[tokio::test]
async fn buy_offer_stale_driver_does_not_overwrite_a_newer_attempt() {
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    env.wallet.complete_latest_transaction_approval().await;
    install_newer_attempt_hook(&env, "receipt");
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(answer["buy_progress"]["resumable"], true, "{answer}");
    let kept = env.market_purchase().unwrap();
    assert_eq!(kept.attempt_id, "fedcba9876543210fedcba9876543210");
    assert!(
        matches!(
            kept.progress,
            crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
                confirmed_buy: None,
                ..
            }
        ),
        "the older attempt's progress must not land on the newer one"
    );
}

/// R25: a declined attempt is dead -- a declined effect is never re-raised or
/// broadcast -- so it is retired, the answer stays the declined outcome, and
/// the next press is a fresh attempt.
#[tokio::test]
async fn buy_offer_retires_a_declined_attempt() {
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    let declined = env.market_purchase().unwrap();
    {
        let mut approvals = env.wallet.approvals.lock().await;
        let approval = approvals
            .iter_mut()
            .rev()
            .find(|approval| approval["intent"] == "transaction_intent")
            .unwrap();
        approval["status"] = json!("rejected");
    }
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["buy_progress"]["stage"], "declined", "{answer}");
    assert_eq!(answer["buy_progress"]["resumable"], false, "{answer}");
    assert!(env.market_purchase().is_none());

    let (_, fresh) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        fresh["buy_progress"]["stage"], "purchase_approval",
        "{fresh}"
    );
    let attempt = env.market_purchase().unwrap();
    assert_ne!(
        attempt.acquisition_stage.effect_id,
        declined.acquisition_stage.effect_id
    );
    assert_eq!(env.wallet_transaction_requests().await, 2);
}

/// The ERC-20 path: the allowance approval is recorded and raised first, the
/// buy after it confirms, and each effect is raised exactly once.
#[tokio::test]
async fn buy_offer_completes_an_erc20_offer_in_two_steps() {
    let env = buy_env().await;
    let body = buy_offer_body(LISTING_SELLER, "0xf4240", LISTING_PAY_TOKEN);
    let (_, first) = env.buy_offer(body.clone()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "allowance_approval",
        "{first}"
    );
    let recorded = env.market_purchase().unwrap();
    assert!(recorded.approval_stage.is_some());
    assert_eq!(
        recorded.offer.payment_processor.as_deref(),
        Some(LISTING_PAYMENT_PROCESSOR)
    );
    assert_eq!(env.wallet_transaction_requests().await, 1);

    env.wallet.complete_latest_transaction_approval().await;
    let (_, second) = env.buy_offer(body.clone()).await;
    assert_eq!(
        second["buy_progress"]["stage"], "purchase_approval",
        "{second}"
    );
    assert_eq!(env.wallet_transaction_requests().await, 2);

    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().has_access = true;
    let (_, done) = env.buy_offer(body.clone()).await;
    assert_eq!(done["status"], "ok", "{done}");
    let (_, again) = env.buy_offer(body).await;
    assert_eq!(again["data"], done["data"]);
    assert_eq!(env.wallet_transaction_requests().await, 2);
}

/// The native offer's recorded terms, as `attempt_in_progress` states them.
fn recorded_native_offer() -> Value {
    json!({
        "seller": LISTING_NATIVE_SELLER,
        "quantity": "0x1",
        "price": LISTING_NATIVE_PRICE,
        "pay_token": LISTING_NATIVE_PAY_TOKEN,
        "payment_processor": null,
    })
}

/// R28 (SM-I1): a press for another seller, or other terms, while a recorded
/// attempt has no confirmed buy, is told which purchase is in progress. It
/// drives nothing: no Wallet request, the record untouched. The recorded
/// terms still resume it.
#[tokio::test]
async fn buy_offer_answers_attempt_in_progress_for_other_terms() {
    let env = buy_env().await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    let recorded = env.market_purchase().unwrap();

    for body in [
        // Another seller of the same item.
        buy_offer_body(LISTING_SELLER, "0xf4240", LISTING_PAY_TOKEN),
        // The same seller at another price.
        buy_offer_body(LISTING_NATIVE_SELLER, "0x1", LISTING_NATIVE_PAY_TOKEN),
        // The same seller and price in another pay token.
        buy_offer_body(
            LISTING_NATIVE_SELLER,
            LISTING_NATIVE_PRICE,
            LISTING_PAY_TOKEN,
        ),
    ] {
        let (status, answer) = env.buy_offer(body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_refused(&answer, "attempt_in_progress");
        assert_eq!(answer["current"], recorded_native_offer(), "{answer}");
        assert!(answer.get("buy_progress").is_none(), "{answer}");
    }
    assert_eq!(
        env.wallet_transaction_requests().await,
        1,
        "a press on other terms must never raise, or re-raise, an effect"
    );
    assert_eq!(env.market_purchase().unwrap(), recorded);

    // The recorded terms resume the one attempt.
    let (_, resumed) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        resumed["buy_progress"]["stage"], "purchase_approval",
        "{resumed}"
    );
    assert_eq!(env.wallet_transaction_requests().await, 1);
}

/// R28 under a race: two first presses for DIFFERENT sellers at once are one
/// attempt. The press that loses the race to record it is told which one is
/// in progress; nothing of its own is raised.
#[tokio::test]
async fn buy_offer_concurrent_presses_for_two_sellers_raise_one_effect() {
    let env = buy_env().await;
    env.chain.lock().unwrap().slow_purchase_read = true;
    let erc20 = buy_offer_body(LISTING_SELLER, "0xf4240", LISTING_PAY_TOKEN);
    let (first, second) = tokio::join!(env.buy_offer(native_buy_body()), env.buy_offer(erc20));
    let answers = [first.1, second.1];
    let waiting = answers
        .iter()
        .filter(|answer| answer.get("buy_progress").is_some())
        .count();
    let refused = answers
        .iter()
        .filter(|answer| answer["code"] == "attempt_in_progress")
        .count();
    assert_eq!((waiting, refused), (1, 1), "{answers:#?}");
    assert_eq!(
        env.wallet_transaction_requests().await,
        1,
        "two presses must never be two payments"
    );
    let recorded = env.market_purchase().unwrap();
    let in_progress = answers
        .iter()
        .find(|answer| answer["code"] == "attempt_in_progress")
        .unwrap();
    assert_eq!(
        in_progress["current"]["seller"].as_str(),
        Some(recorded.offer.seller.as_str()),
        "the refusal names the attempt that was recorded"
    );
}

/// R28's limit: once the recorded attempt's buy is confirmed on chain, the
/// money has moved. Any press finishes that purchase, whatever it names.
#[tokio::test]
async fn buy_offer_finishes_a_confirmed_buy_whatever_the_press_names() {
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    // An attempt that knows its KID completes on the chain's access grant,
    // so it can be confirmed and still waiting.
    record_the_kid(&env);
    env.wallet.complete_latest_transaction_approval().await;
    // The buy confirms; the grant is not readable yet.
    let (_, waiting) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        waiting["buy_progress"]["stage"], "access_evidence",
        "{waiting}"
    );
    env.chain.lock().unwrap().has_access = true;
    let (_, done) = env
        .buy_offer(buy_offer_body(LISTING_SELLER, "0xf4240", LISTING_PAY_TOKEN))
        .await;
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(env.wallet_transaction_requests().await, 1);
}

/// A listing-package purchase of the item this Home's person has begun
/// (`buy`), recorded exactly as that path records one before its first effect.
fn pending_listing_purchase(
    principal_id: &str,
    address: &str,
) -> crate::protected_content_runtime::RuntimeCustodyPurchaseRecord {
    let stage = crate::protected_content_runtime::RuntimeCustodyPurchaseStageRecord {
        stage: "buy".to_string(),
        effect_id: "runtime-effect:22222222222222222222222222222222".to_string(),
        approval_request_id: "wallet-request:22222222222222222222222222222222".to_string(),
        request_sha256: format!("sha256:{}", "44".repeat(32)),
        chain_namespace: "eip155:8453".to_string(),
        network: "base-mainnet".to_string(),
        to: LISTING_MARKET_CONTRACT.to_string(),
        value: LISTING_NATIVE_PRICE.to_string(),
        data: LISTING_MARKET_BUY_DATA.to_string(),
    };
    crate::protected_content_runtime::RuntimeCustodyPurchaseRecord {
        schema: crate::protected_content_runtime::RUNTIME_PURCHASE_SCHEMA_V1.to_string(),
        principal_id: principal_id.to_string(),
        profile_did: "did:key:z6MkListingPurchaseFixture".to_string(),
        mint_id: "ab".repeat(32),
        content_id: "content:listing-purchase-fixture".to_string(),
        cid: LISTING_COVER.to_string(),
        listing_sha256: format!("sha256:{}", "55".repeat(32)),
        seller_address: LISTING_NATIVE_SELLER.to_string(),
        chain_namespace: "eip155:8453".to_string(),
        network: "base-mainnet".to_string(),
        // The listing names the item in its own spelling; the key compares
        // without regard to case.
        ledger: LISTING_LEDGER.to_ascii_uppercase().replacen("0X", "0x", 1),
        token_id: LISTING_TOKEN_ID.to_string(),
        operative: LISTING_OPERATIVE.to_string(),
        price: LISTING_NATIVE_PRICE.to_string(),
        pay_token: LISTING_NATIVE_PAY_TOKEN.to_string(),
        payment_processor: None,
        availability_receipt_digest: format!("sha256:{}", "66".repeat(32)),
        account_id: "wallet-account-listing-fixture".to_string(),
        address: address.to_string(),
        approval_stage: None,
        acquisition_stage: stage,
        acquisition: crate::protected_content_runtime::RuntimeCustodyAcquisitionV1::Bought,
        capsule_uri: None,
        progress: crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending {
            confirmed_approval: None,
            confirmed_buy: None,
        },
        created_at: 1,
        updated_at: 1,
    }
}

/// The same listing-package purchase, completed: this person owns the item.
fn completed_listing_purchase(
    principal_id: &str,
    address: &str,
) -> crate::protected_content_runtime::RuntimeCustodyPurchaseRecord {
    let mut purchase = pending_listing_purchase(principal_id, address);
    purchase.progress =
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete {
            terminal: super::library::completed_market_terminal_for_test(),
        };
    purchase
}

/// Record the item's KID on the recorded attempt, as an attempt recorded
/// before R46 names it.
fn record_the_kid(env: &ListingEnv) {
    let mut recorded = serde_json::to_value(env.market_purchase().unwrap()).unwrap();
    recorded["item"]["kid"] = json!(LISTING_KID);
    crate::protected_content_market::persist_runtime_market_purchase(
        env._dir.path(),
        &serde_json::from_value(recorded).unwrap(),
    )
    .unwrap();
}

/// R29 (SM-I2): one purchase per item across both paths. A listing-package
/// purchase of this item already under way refuses a market attempt before
/// anything is read, recorded or raised.
#[tokio::test]
async fn buy_offer_refuses_an_item_the_listing_path_is_buying() {
    let env = buy_env().await;
    crate::protected_content_runtime::persist_runtime_custody_purchase(
        env._dir.path(),
        &pending_listing_purchase(&env.authority.principal_id, &env.buyer_address),
    )
    .unwrap();
    let (status, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_refused(&answer, "attempt_in_progress");
    assert_eq!(answer["current"], Value::Null, "{answer}");
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());

    // Another person's listing purchase on the same Home is not this one's.
    let env = buy_env().await;
    crate::protected_content_runtime::persist_runtime_custody_purchase(
        env._dir.path(),
        &pending_listing_purchase("someone-else", &env.buyer_address),
    )
    .unwrap();
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        answer["buy_progress"]["stage"], "purchase_approval",
        "{answer}"
    );
}

/// R43 (m2): a listing-package purchase record this Home cannot read may be
/// a purchase under way, so the buy is refused as unavailable -- nothing
/// raised, nothing recorded -- rather than started beside it.
#[cfg(unix)]
#[tokio::test]
async fn buy_offer_is_unavailable_while_the_purchase_ledger_is_unreadable() {
    use std::os::unix::fs::PermissionsExt as _;
    let env = buy_env().await;
    let recorded = pending_listing_purchase(&env.authority.principal_id, &env.buyer_address);
    crate::protected_content_runtime::persist_runtime_custody_purchase(env._dir.path(), &recorded)
        .unwrap();
    let record_path = walk_files(env._dir.path())
        .into_iter()
        .find(|path| path.ends_with(format!("{}.json", recorded.mint_id)))
        .unwrap();
    std::fs::set_permissions(&record_path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (status, answer) = env.buy_offer(native_buy_body()).await;
    std::fs::set_permissions(&record_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(answer["code"], "library_error", "{answer}");
    assert_eq!(
        answer["message"],
        crate::protected_content_runtime::RUNTIME_CUSTODY_PURCHASE_UNAVAILABLE_MESSAGE,
        "{answer}"
    );
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

fn walk_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// R33 (PS-M11): "your own offer" is answered only about a seller the chain
/// shows holding a live offer. Any other address is simply no offer, so the
/// page cannot use `buy_offer` to test whether an address is this Home's.
#[tokio::test]
async fn buy_offer_answers_own_offer_only_for_a_live_seller() {
    let env = buy_env().await;
    let own = env.buyer_address.to_ascii_lowercase();
    let (status, answer) = env
        .buy_offer(buy_offer_body(
            &own,
            LISTING_NATIVE_PRICE,
            LISTING_NATIVE_PAY_TOKEN,
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_refused(&answer, "terms_changed");
    assert_eq!(answer["current"], Value::Null, "{answer}");
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

/// R35 (PS-M8): an ERC-20 offer the chain names no processor for is never
/// stated -- the listing refuses it, and so does `terms_changed`.
#[tokio::test]
async fn buy_offer_terms_changed_never_states_an_erc20_offer_without_a_processor() {
    let env = buy_env().await;
    env.chain.lock().unwrap().offers = Some(json!([{
        "seller": LISTING_SELLER,
        "quantity": "0x2710",
        "price": "0xf4240",
        "pay_token": LISTING_PAY_TOKEN,
    }]));
    let (_, answer) = env
        .buy_offer(buy_offer_body(LISTING_SELLER, "0x1", LISTING_PAY_TOKEN))
        .await;
    assert_refused(&answer, "terms_changed");
    assert_eq!(answer["current"], Value::Null, "{answer}");
    assert_eq!(env.wallet_transaction_requests().await, 0);
}

/// R31 (SM-I4): a node's own sentence can name the buyer's account. No
/// Marketplace buy answer carries it -- not in `message`, not in `detail`.
#[tokio::test]
async fn buy_offer_answers_never_carry_an_account_address() {
    let env = buy_env().await;
    env.buy_offer(native_buy_body()).await;
    env.wallet.complete_latest_transaction_approval().await;
    env.chain.lock().unwrap().broadcast_error = Some(format!(
        "EVM RPC request rejected: eth_sendRawTransaction: code -32000: insufficient funds for gas * price + value: address {} have 0 want 10000000000000000",
        env.buyer_address
    ));
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    let bare = env
        .buyer_address
        .trim_start_matches("0x")
        .to_ascii_lowercase();
    let serialized = serde_json::to_string(&answer).unwrap().to_ascii_lowercase();
    assert!(
        !serialized.contains(&bare),
        "the page must never learn this Home's account: {serialized}"
    );
    assert!(
        answer["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("insufficient funds")),
        "the cause is still told, without the address: {answer}"
    );
}

// --- R46: a purchase rests on the chain's terms only ------------------------

/// The item's own folder, holding its shared document, which this Home
/// cannot read (a gateway timeout).
fn unreadable_document() -> ListingContentFixture {
    let mut content = content_with(
        LISTING_FOLDER,
        "metadata.json",
        &elastos_v1_metadata(LISTING_KID),
    );
    content.unreadable_paths.push("metadata.json".to_string());
    content
}

/// Every request the chain was asked with the op `op`.
fn chain_ops(env: &ListingEnv, op: &str) -> Vec<Value> {
    env.chain
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| request["op"] == op)
        .cloned()
        .collect()
}

/// The item's KID as its purchase record states it, `None` when unknown.
fn recorded_kid(env: &ListingEnv) -> Option<Value> {
    let record = env.market_purchase().expect("a recorded market purchase");
    serde_json::to_value(&record.item)
        .unwrap()
        .get("kid")
        .cloned()
        .filter(|kid| !kid.is_null())
}

/// Buy, approve in the Wallet, and press again: the completed answer.
async fn buy_and_complete(env: &ListingEnv) -> Value {
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    env.wallet.complete_latest_transaction_approval().await;
    // The buy landed: the chain grants the buyer access to the item.
    env.chain.lock().unwrap().has_access = true;
    let (status, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "ok", "{answer}");
    answer["data"].clone()
}

/// R46: "content availability nor a proof of CEK recovering ability should
/// not block purchase". A shared document this Home cannot read, or a KID
/// binding the chain cannot answer, leaves the buy exactly where the chain's
/// terms put it: at the Wallet.
#[tokio::test]
async fn buy_offer_reaches_the_wallet_whatever_the_document_and_kid_binding_say() {
    let mut binding_unavailable = chain_with_item(LISTING_FOLDER, LISTING_KID);
    binding_unavailable.kid_binding_unavailable = true;
    let cases = vec![
        (
            "document unreadable",
            chain_with_item(LISTING_FOLDER, LISTING_KID),
            unreadable_document(),
        ),
        (
            "document absent",
            chain_with_item(LISTING_FOLDER, LISTING_KID),
            ListingContentFixture::default(),
        ),
        (
            "binding unavailable",
            binding_unavailable,
            content_with(
                LISTING_FOLDER,
                "metadata.json",
                &elastos_v1_metadata(LISTING_KID),
            ),
        ),
    ];
    for (case, chain, content) in cases {
        let env = ListingEnv::new(chain, content).await;
        let (status, answer) = env.buy_offer(native_buy_body()).await;
        assert_eq!(status, StatusCode::OK, "{case}: {answer}");
        assert_eq!(
            answer["buy_progress"]["stage"], "purchase_approval",
            "{case}: {answer}"
        );
        assert_eq!(env.wallet_transaction_requests().await, 1, "{case}");
        assert!(env.market_purchase().is_some(), "{case}");
    }
}

/// R46: the buy itself never asks the chain for the KID binding and never
/// reads the shared document; a KID the page sends is not a precondition.
#[tokio::test]
async fn buy_offer_never_reads_the_kid_binding_nor_the_shared_document() {
    let env = buy_env().await;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        answer["buy_progress"]["stage"], "purchase_approval",
        "{answer}"
    );
    assert!(chain_ops(&env, "resolve_protected_content_kid_binding").is_empty());
    let paths = env.content_paths();
    assert!(paths.is_empty(), "a buy reads no document: {paths:?}");
    assert_eq!(recorded_kid(&env), None, "the attempt records no KID");

    // Another item's KID, sent by the page, gates nothing.
    let env = buy_env().await;
    let mut body = native_buy_body();
    body["item"]["kid"] = json!(LISTING_OTHER_KID);
    let (_, answer) = env.buy_offer(body).await;
    assert_eq!(
        answer["buy_progress"]["stage"], "purchase_approval",
        "{answer}"
    );
    assert_eq!(recorded_kid(&env), None);
}

/// R46: the purchase completes on the chain's own receipt with the KID
/// unknown -- `item.kid` is `null` -- and adoption, after payment, learns the
/// KID from the shared document once it is readable, checks it against the
/// chain's binding, and only then records it.
#[tokio::test]
async fn buy_offer_completes_with_an_unknown_kid_and_adoption_learns_it() {
    let env = ListingEnv::new(
        chain_with_item(LISTING_FOLDER, LISTING_KID),
        unreadable_document(),
    )
    .await;
    let complete = buy_and_complete(&env).await;
    assert_eq!(complete["item"]["kid"], Value::Null, "{complete}");
    assert_eq!(complete["item"]["operative"], LISTING_OPERATIVE);
    assert_eq!(complete["adoption"], "pending", "{complete}");
    assert!(matches!(
        env.market_purchase().unwrap().progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { .. }
    ));
    assert_eq!(recorded_kid(&env), None);
    assert!(chain_ops(&env, "resolve_protected_content_kid_binding").is_empty());
    assert_eq!(env.wallet_transaction_requests().await, 1);

    env.content.lock().unwrap().unreadable_paths.clear();
    env.chain.lock().unwrap().has_access = true;
    let (_, again) = env.buy_offer(native_buy_body()).await;
    assert_eq!(again["status"], "ok", "{again}");
    assert_eq!(again["data"]["item"]["kid"], LISTING_KID, "{again}");
    let bindings = chain_ops(&env, "resolve_protected_content_kid_binding");
    assert!(!bindings.is_empty(), "adoption checks the binding");
    assert!(bindings
        .iter()
        .all(|request| request["content_access_id"] == LISTING_KID));
    let access = chain_ops(&env, "resolve_protected_content_purchase_access");
    assert!(
        !access.is_empty(),
        "adoption reads the access with that KID"
    );
    assert!(access
        .iter()
        .all(|request| request["content_access_id"] == LISTING_KID));
    assert_eq!(recorded_kid(&env), Some(json!(LISTING_KID)));
    // R50: the completion's item-keyed evidence gave way to the KID's own
    // answer, the one the open path asks for.
    let record = env.market_purchase().unwrap();
    let crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { terminal } =
        &record.progress
    else {
        panic!("complete");
    };
    let evidence = terminal.access_evidence.as_ref().expect("access evidence");
    assert_eq!(evidence.content_access_id.as_deref(), Some(LISTING_KID));
    assert_eq!(
        (evidence.ledger.as_deref(), evidence.token_id.as_deref()),
        (None, None)
    );
    assert_eq!(env.wallet_transaction_requests().await, 1);
}

/// R46: a document whose KID the chain binds to another item is a proven
/// mismatch. Adoption refuses it as it refuses a foreign asset, records no
/// KID and writes no mint.
#[tokio::test]
async fn adoption_refuses_a_document_whose_kid_the_chain_binds_elsewhere() {
    let mut chain = chain_with_item(LISTING_FOLDER, LISTING_KID);
    chain.bindings.insert(
        LISTING_KID.to_string(),
        (
            LISTING_LEDGER.to_string(),
            LISTING_OTHER_TOKEN_ID.to_string(),
        ),
    );
    let env = ListingEnv::new(chain, unreadable_document()).await;
    buy_and_complete(&env).await;

    env.content.lock().unwrap().unreadable_paths.clear();
    env.chain.lock().unwrap().has_access = true;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(answer["data"]["adoption"], "foreign", "{answer}");
    assert_eq!(answer["data"]["item"]["kid"], Value::Null, "{answer}");
    let record = env.market_purchase().unwrap();
    assert_eq!(record.adopted_mint_id, None);
    assert_eq!(recorded_kid(&env), None);
    assert!(
        crate::protected_content_runtime::runtime_custody_listing_chain_index(
            env._dir.path(),
            &env.authority.principal_id,
        )
        .unwrap()
        .is_empty(),
        "a mismatched document gets no listing record"
    );
}

/// R46: a KID adoption cannot check against the chain stays unknown, and
/// adoption stays `pending` for the next answer (or Download copy) to retry.
#[tokio::test]
async fn adoption_stays_pending_while_the_kid_binding_cannot_be_read() {
    let env = ListingEnv::new(
        chain_with_item(LISTING_FOLDER, LISTING_KID),
        unreadable_document(),
    )
    .await;
    buy_and_complete(&env).await;

    env.content.lock().unwrap().unreadable_paths.clear();
    {
        let mut chain = env.chain.lock().unwrap();
        chain.kid_binding_unavailable = true;
        chain.has_access = true;
    }
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "ok", "{answer}");
    assert_eq!(answer["data"]["adoption"], "pending", "{answer}");
    assert_eq!(answer["data"]["item"]["kid"], Value::Null, "{answer}");
    assert_eq!(recorded_kid(&env), None);
}

/// An attempt recorded before R46 names its KID. It resumes on that KID:
/// the access evidence is asked with it, as before.
#[tokio::test]
async fn buy_offer_finishes_an_attempt_recorded_with_its_kid() {
    let env = buy_env().await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    // The pre-payment ownership check knew no KID yet: it asked by the item.
    let item_reads_before_the_kid = chain_ops(&env, "resolve_protected_content_item_access").len();
    let mut recorded = serde_json::to_value(env.market_purchase().unwrap()).unwrap();
    recorded["item"]["kid"] = json!(LISTING_KID);
    crate::protected_content_market::persist_runtime_market_purchase(
        env._dir.path(),
        &serde_json::from_value(recorded).unwrap(),
    )
    .unwrap();
    env.wallet.complete_latest_transaction_approval().await;

    // No grant yet: the recorded KID's access is what completion waits on.
    let (_, waiting) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        waiting["buy_progress"]["stage"], "access_evidence",
        "{waiting}"
    );
    env.chain.lock().unwrap().has_access = true;
    let (_, done) = env.buy_offer(native_buy_body()).await;
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(done["data"]["item"]["kid"], LISTING_KID, "{done}");
    let access = chain_ops(&env, "resolve_protected_content_purchase_access");
    assert!(!access.is_empty());
    assert!(access
        .iter()
        .all(|request| request["content_access_id"] == LISTING_KID));
    // R50: once the KID is known the content-id read answers; the
    // item-keyed read is only for an item whose KID is unknown.
    assert_eq!(
        chain_ops(&env, "resolve_protected_content_item_access").len(),
        item_reads_before_the_kid
    );
    let record = env.market_purchase().unwrap();
    let crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { terminal } =
        &record.progress
    else {
        panic!("complete");
    };
    let evidence = terminal.access_evidence.as_ref().expect("access evidence");
    assert_eq!(evidence.content_access_id.as_deref(), Some(LISTING_KID));
    assert_eq!(evidence.ledger, None);
}

/// R46: the offers are the terms a buy is made on. A chain that cannot state
/// them is still an unavailable purchase, as the page knows it: nothing is
/// raised and nothing is recorded.
#[tokio::test]
async fn buy_offer_is_unavailable_while_the_offers_cannot_be_read() {
    let mut chain = chain_with_item(LISTING_FOLDER, LISTING_KID);
    chain.offers_unavailable = true;
    let env = ListingEnv::new(chain, unreadable_document()).await;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["status"], "error", "{answer}");
    assert_eq!(answer["code"], "library_error", "{answer}");
    assert_eq!(
        answer["message"], "Runtime custody purchase is unavailable",
        "{answer}"
    );
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

/// The item-keyed access reads this env's chain was asked, as
/// `(ledger, token_id, block)`.
fn item_access_reads(env: &ListingEnv) -> Vec<(Value, Value, Value)> {
    chain_ops(env, "resolve_protected_content_item_access")
        .into_iter()
        .map(|request| {
            (
                request["ledger"].clone(),
                request["token_id"].clone(),
                request["block"].clone(),
            )
        })
        .collect()
}

/// R50: this person already holds the item through another Home (or
/// ela.city) -- no record here says so, and the KID is unknown -- yet the
/// chain grants their buyer account access to the item. The press is refused
/// as `already_owned` before anything is raised: never a second payment.
#[tokio::test]
async fn buy_offer_refuses_an_item_owned_through_another_home_with_an_unknown_kid() {
    let mut chain = chain_with_item(LISTING_FOLDER, LISTING_KID);
    chain.has_access = true;
    let env = ListingEnv::new(chain, unreadable_document()).await;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_refused(&answer, "already_owned");
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
    assert_eq!(
        item_access_reads(&env),
        vec![(
            json!(LISTING_LEDGER),
            json!(LISTING_TOKEN_ID),
            json!("latest")
        )]
    );
    let asked = chain_ops(&env, "resolve_protected_content_item_access");
    assert!(asked[0]["wallet"]
        .as_str()
        .unwrap()
        .eq_ignore_ascii_case(&env.buyer_address));
    assert!(chain_ops(&env, "resolve_protected_content_purchase_access").is_empty());
    assert!(chain_ops(&env, "resolve_protected_content_kid_binding").is_empty());

    // A chain that cannot answer fails closed: nothing starts on an unknown.
    let mut chain = chain_with_item(LISTING_FOLDER, LISTING_KID);
    chain.access_unanswered = true;
    let env = ListingEnv::new(chain, unreadable_document()).await;
    let (_, answer) = env.buy_offer(native_buy_body()).await;
    assert_eq!(answer["code"], "library_error", "{answer}");
    assert_eq!(env.wallet_transaction_requests().await, 0);
    assert!(env.market_purchase().is_none());
}

/// R50: a purchase whose KID is unknown completes on the chain's access
/// answer for the item -- not on the mined receipt alone -- and its terminal
/// record carries that evidence, keyed by the item.
#[tokio::test]
async fn buy_offer_completes_a_kidless_purchase_on_item_access_evidence() {
    let env = ListingEnv::new(
        chain_with_item(LISTING_FOLDER, LISTING_KID),
        unreadable_document(),
    )
    .await;
    let (_, first) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        first["buy_progress"]["stage"], "purchase_approval",
        "{first}"
    );
    env.wallet.complete_latest_transaction_approval().await;

    // Mined with status 0x1, but the chain grants no access yet: waiting.
    let (_, waiting) = env.buy_offer(native_buy_body()).await;
    assert_eq!(
        waiting["buy_progress"]["stage"], "access_evidence",
        "{waiting}"
    );
    assert!(matches!(
        env.market_purchase().unwrap().progress,
        crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Pending { .. }
    ));

    env.chain.lock().unwrap().has_access = true;
    let (_, done) = env.buy_offer(native_buy_body()).await;
    assert_eq!(done["status"], "ok", "{done}");
    assert_eq!(done["data"]["item"]["kid"], Value::Null, "{done}");
    let record = env.market_purchase().unwrap();
    let crate::protected_content_runtime::RuntimeCustodyPurchaseProgress::Complete { terminal } =
        &record.progress
    else {
        panic!("complete: {:?}", record.progress);
    };
    let evidence = terminal
        .access_evidence
        .as_ref()
        .expect("a KID-less completion carries access evidence");
    assert!(evidence.has_access);
    assert_eq!(
        evidence.schema,
        "elastos.chain.protected-content-item-access/v1"
    );
    assert_eq!(evidence.ledger.as_deref(), Some(LISTING_LEDGER));
    assert_eq!(evidence.token_id.as_deref(), Some(LISTING_TOKEN_ID));
    assert_eq!(evidence.content_access_id, None);
    assert!(evidence.wallet.eq_ignore_ascii_case(&env.buyer_address));
    assert_eq!(evidence.chain_id, 8453);
    assert!(item_access_reads(&env)
        .iter()
        .all(|(ledger, token_id, block)| *ledger == LISTING_LEDGER
            && *token_id == LISTING_TOKEN_ID
            && *block == "latest"));
    assert!(chain_ops(&env, "resolve_protected_content_purchase_access").is_empty());
    assert_eq!(env.wallet_transaction_requests().await, 1);
}
