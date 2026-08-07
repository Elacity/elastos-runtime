//! E2E ANCHOR: the dKMS rail must hold end-to-end on the ESP substrate.
//!
//! The rail is `publish → index → buy → acquire → viewer open → session-bound media → close →
//! sweeper reap`, and its defining property is that EVERY step is fail-closed: nothing before a
//! completed acquisition opens, and nothing outside the session's own bearer reads it.
//!
//! SCOPE, stated honestly. The port's per-leg branches already have dedicated ledgers — the
//! money-verb step-up branches in `marketplace.rs`, the store bounds in `session_bounds`, the
//! lifecycle dispatch in `session_lifecycle`. This file does NOT re-prove them. It proves the
//! JOIN: that the legs are actually wired to each other, in order, with the refusal at each seam.
//!
//! What is deliberately NOT exercised here: the real decrypt legs (`ddrm-media-authority` →
//! `decrypt-provider` → `key-provider`) and the live rights capsule. Those are separate binaries
//! that the default `cargo test -p elastos-server --lib` gate does not build, and in a default
//! (non-`dev-modes`) build `rights_mode()` is pinned to the real `Chain` path. Standing up a
//! shell-stub of the key authority would prove the stub, not the rail. So the media leg is
//! anchored where the rail's AUTHORITY actually lives: at `MediaSession` admission and the read
//! gate that gets a browser to bytes. The crypto legs are pinned in the capsule crates' own
//! suites (`just verify-capsules`) and in `tests/ddrm_verdicts.rs`.

use super::*;
use crate::api::viewer_media::{MediaSession, MEDIA_VIEWER_CAPSULE};

// ── rail fixtures ──────────────────────────────────────────────────────────────

/// A second, INDEPENDENT admin principal. `passkey_authority` pins one credential id, so every
/// call yields the same principal — a cross-principal isolation claim needs two, and both must
/// be Admin or the refusal could be a role decision rather than the ownership decision under test.
fn rail_authority(data_dir: &std::path::Path, credential_id: &str) -> TestPasskeyAuthority {
    let now = crate::auth::now_ts();
    let rp_id = "elastos.elacitylabs.com";
    let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
        credential_id: credential_id.to_string(),
        public_key: format!("{credential_id}-public-key"),
        sign_count: 1,
        user_verified: true,
        origin: "https://elastos.elacitylabs.com".to_string(),
        rp_id: rp_id.to_string(),
        created_at: now,
        last_used_at: now,
        revoked_at: None,
    });
    let principal_id = crate::auth::passkey_credential_principal_id(rp_id, credential_id).unwrap();
    let principal = crate::auth::upsert_principal_for_binding_as_role_named(
        data_dir,
        binding,
        principal_id,
        crate::auth::RuntimePrincipalRole::Admin,
        None,
        now,
    )
    .unwrap();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", uuid_like_token()),
        session_id: format!("auth:{}", uuid_like_token()),
        principal_id: principal.principal_id.clone(),
        proof_binding_id: principal.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec![
            HOME_CAPSULE_ID.to_string(),
            PEOPLE_CAPSULE_ID.to_string(),
            SYSTEM_CAPSULE_ID.to_string(),
            // The viewer the rail launches. A launch token is only honored when the AUTH GRANT
            // behind it already covers that capsule — so the player's credential descends from
            // the human's authenticated session, it is not conjured at open time.
            MEDIA_VIEWER_CAPSULE.to_string(),
        ],
    };
    crate::auth::store_session_grant(data_dir, grant.clone()).unwrap();
    TestPasskeyAuthority {
        home_token: issue_home_launch_token_for_auth_grant(data_dir, HOME_CAPSULE_ID, &grant)
            .unwrap(),
        people_token: issue_home_launch_token_for_auth_grant(data_dir, PEOPLE_CAPSULE_ID, &grant)
            .unwrap(),
        system_token: issue_home_launch_token_for_auth_grant(data_dir, SYSTEM_CAPSULE_ID, &grant)
            .unwrap(),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        session_id: grant.session_id,
        grant_id: grant.grant_id,
    }
}

/// A well-formed `DigitalAssetRegistered` log — the PUBLISH event the creator's mint emits. This
/// is the shape that carries the KID (`bytes16 contentId`) on-chain, so it is the one that lets a
/// buyer address the asset by content id without a metadata round-trip.
///
/// `data = abi.encode(address creator, string tokenURI, uint16 opType, bytes16 contentId)`:
/// head words `[creator][uri-offset][opType][contentId]`, then the string tail at the offset.
fn published_asset_log(
    channel: &str,
    token_id: u64,
    creator: &str,
    token_uri: &str,
    op_type: u64,
    content_id_hex32: &str,
    block: u64,
) -> Value {
    let pad_addr = |a: &str| format!("{:0>64}", a.trim_start_matches("0x").to_lowercase());
    let uri_hex: String = token_uri.bytes().map(|b| format!("{b:02x}")).collect();
    let uri_len = format!("{:064x}", token_uri.len());
    let uri_padded = format!(
        "{uri_hex:0<width$}",
        width = token_uri.len().div_ceil(32) * 64
    );
    // 4 head words ⇒ the string tail starts at byte 128.
    let head = format!(
        "{}{}{}{}",
        pad_addr(creator),
        format_args!("{:064x}", 128),
        format_args!("{op_type:064x}"),
        // bytes16 is LEFT-aligned in its word: 32 hex of KID + 32 hex of padding.
        format_args!("{content_id_hex32:0<64}"),
    );
    json!({
        "topics": [
            crate::api::content_index::DIGITAL_ASSET_REGISTERED_TOPIC0,
            pad_addr(channel),
            format!("0x{token_id:064x}"),
        ],
        "data": format!("0x{head}{uri_len}{uri_padded}"),
        "blockNumber": format!("0x{block:x}"),
    })
}

/// A media session shaped exactly as a completed open registers one: viewer-scoped,
/// principal-bound, clear init bytes, NO key material (`decrypt_request`/`sealed_material` are the
/// relay envelope, not the CEK — the CEK never reaches the gateway on any path).
fn rail_media_session(principal_id: &str, expires_at: u64) -> MediaSession {
    MediaSession {
        viewer: MEDIA_VIEWER_CAPSULE.to_string(),
        principal_id: principal_id.to_string(),
        mime: "video/mp4; codecs=\"avc1.640028\"".to_string(),
        segment_count: 3,
        has_init: true,
        init_bytes: b"rail-anchor-init-segment".to_vec(),
        is_protected: true,
        expires_at,
        decrypt_request: Value::Null,
        sealed_material: Value::Null,
        authority: None,
        tracks: Vec::new(),
        cover_uri: None,
        title: "Rail Anchor Asset".to_string(),
    }
}

/// A read against a media route, carrying a viewer launch token in the header (the launch token
/// itself arrives at the capsule in the URL FRAGMENT; the capsule then sends it as a header —
/// so a media request is header-borne by construction, never query-borne).
fn media_get(uri: &str, viewer_token: Option<&str>) -> Request<Body> {
    let request = test_browser_request("localhost:61180", "null").uri(uri);
    let request = match viewer_token {
        Some(token) => request.header("x-elastos-home-token", token),
        None => request,
    };
    request.body(Body::empty()).unwrap()
}

async fn rail_body(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn rail_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&rail_body(response).await).unwrap()
}

async fn rail_text(response: axum::response::Response) -> String {
    String::from_utf8_lossy(&rail_body(response).await).into_owned()
}

// ── 1. publish → index → buy ───────────────────────────────────────────────────

/// STEPS 1–3. A creator's publish event is what the index reads, and the indexed row is what the
/// buyer addresses the money verb with. This walks that handoff end to end: the KID that comes out
/// of the on-chain publish event is the SAME KID the buy is authorized against — no second source
/// of truth, no metadata detour in the middle.
///
/// The money-verb gate itself (freshness, intent binding, single use, opaque refusals) is proven
/// in `marketplace.rs`; here it is a link in the chain, so the assertion is only the one the rail
/// needs: an acquisition carrying a fresh step-up bound to the INDEXED asset reaches the buy.
#[tokio::test]
async fn dkms_rail_publish_indexes_the_asset_and_the_indexed_kid_drives_the_buy() {
    let dir = tempfile::tempdir().unwrap();
    let buyer = passkey_authority(dir.path());
    let app = gateway_router(test_state(dir.path()));

    // 1. PUBLISH — the creator's mint emits the asset's registration, KID and all.
    const KID: &str = "0f0e0d0c0b0a09080706050403020100";
    let channel = "0x6756e1407164ae34f8df5334d48d0e45c094b8b9";
    let creator = "0x1111111111111111111111111111111111111111";
    let log = published_asset_log(
        channel,
        7,
        creator,
        "ipfs://bafy/rail-anchor.json",
        2,
        KID,
        0x2a0,
    );

    // 2. INDEX — the content index picks the publish up, with no help from the minter.
    let mut index = crate::api::content_index::ContentIndex::new();
    let ingested = index.ingest_logs(std::slice::from_ref(&log));
    assert_eq!(ingested, 1, "the publish event must be indexable");

    let listings = index.search(None, None, Some(channel));
    assert_eq!(
        listings.len(),
        1,
        "the published asset must be discoverable on its channel"
    );
    let listed = listings[0].to_json();
    assert_eq!(listed["channel_address"], json!(channel));
    assert_eq!(listed["creator_address"], json!(creator));
    assert_eq!(listed["token_uri"], json!("ipfs://bafy/rail-anchor.json"));
    let indexed_content_id = listed["content_id"]
        .as_str()
        .expect("a registration event carries the KID on-chain")
        .to_string();
    assert_eq!(
        indexed_content_id,
        format!("0x{KID}"),
        "the indexed KID must be the one the publish emitted, byte for byte"
    );

    // 3. BUY — the acquisition is addressed by the INDEXED KID (bare, the `.ddrm` capsule's
    //    at-rest form), and carries a step-up bound to exactly those terms.
    let intent = json!({
        "content_id": indexed_content_id.trim_start_matches("0x"),
        "quantity": "1",
        "expected_price": "1000000000000000000",
    });
    let step_up = step_up_token_for_app_context(
        dir.path(),
        HOME_CAPSULE_ID,
        &buyer.home_token,
        "market.buy",
        &intent,
    );
    let mut approved = intent.clone();
    approved["step_up_token"] = json!(step_up);

    let bought = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/market/buy")
                .header("x-elastos-home-token", buyer.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(approved.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    // "Reached the buy" is about WHICH check answers, not the status: with no linked EVM wallet
    // the acquisition still stops at the wallet-link check (and in a dev-rights build it completes
    // outright). What must not happen is the money-verb gate refusing a step-up that is fresh and
    // bound to the indexed asset — so the assertion is that NONE of the gate's three refusals is
    // the answer, tested against the constants rather than a substring (a substring check would be
    // satisfied by a regression that refused every buy as "not signed in").
    let text = rail_text(bought).await;
    for refusal in [
        crate::api::viewer_open::MONEY_VERB_NOT_SIGNED_IN,
        crate::api::viewer_open::MONEY_VERB_STEP_UP_REQUIRED,
        crate::api::viewer_open::MONEY_VERB_STEP_UP_REJECTED,
    ] {
        assert_ne!(
            text.trim(),
            refusal,
            "a step-up bound to the INDEXED asset must reach the buy"
        );
    }
}

// ── 2. acquire → open (the fail-closed seam) ───────────────────────────────────

/// STEP 4, the refusal half. The rail's whole point is that publishing is public and OPENING is
/// not. A principal who has not acquired the asset must not open it — and the refusal must not
/// double as an oracle: the answer for "someone else's asset" is the same answer as "no such
/// asset", so a token holder cannot walk the node's Library by status code.
#[tokio::test]
async fn dkms_rail_open_fails_closed_before_acquisition() {
    let dir = tempfile::tempdir().unwrap();
    let creator = rail_authority(dir.path(), "dkms-rail-creator-passkey");
    let buyer = rail_authority(dir.path(), "dkms-rail-buyer-passkey");
    assert_ne!(
        creator.principal_id, buyer.principal_id,
        "the isolation claim needs two genuinely distinct principals"
    );
    let app = gateway_router(test_state(dir.path()));

    // The creator holds a REAL asset in their own Library root — so the buyer's refusal below is
    // provably an ownership decision, not the trivial "the file does not exist".
    let creator_root = crate::auth::principal_localhost_root(&creator.principal_id);
    let creator_uri = format!("{creator_root}/Videos/rail-anchor.mp4");
    crate::library::handle_library_upload_bytes(
        dir.path(),
        &creator.principal_id,
        &creator_uri,
        Some("video/mp4"),
        None,
        b"rail-anchor-protected-bytes",
    )
    .expect("the creator can publish into their own Library root");

    let open = |token: Option<&str>, uri: &str| {
        let request = test_browser_request("localhost:61180", "http://localhost:61180")
            .method("POST")
            .uri("/api/viewers/open")
            .header(CONTENT_TYPE, "application/json");
        let request = match token {
            Some(token) => request.header("x-elastos-home-token", token),
            None => request,
        };
        request
            .body(Body::from(json!({ "uri": uri }).to_string()))
            .unwrap()
    };

    // (a) The creator's own asset, opened by the BUYER who never acquired it.
    let denied = app
        .clone()
        .oneshot(open(Some(&buyer.home_token), &creator_uri))
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::NOT_FOUND,
        "an unacquired asset must not open"
    );
    let denied_text = rail_text(denied).await;
    assert!(
        !denied_text.contains("session"),
        "a refused open must mint no viewer session: {denied_text}"
    );
    for leak in [
        creator.principal_id.as_str(),
        creator_root.as_str(),
        dir.path().to_string_lossy().as_ref(),
    ] {
        assert!(
            !denied_text.contains(leak),
            "the refusal leaked the owner's identity or layout ({leak}): {denied_text}"
        );
    }

    // (b) The SAME refusal for an asset that simply does not exist — the two are indistinguishable,
    //     so the route is not an existence oracle for other principals' libraries.
    let absent = app
        .clone()
        .oneshot(open(
            Some(&buyer.home_token),
            &format!(
                "{}/Videos/rail-anchor.mp4",
                crate::auth::principal_localhost_root(&buyer.principal_id)
            ),
        ))
        .await
        .unwrap();
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        rail_text(absent).await,
        denied_text,
        "\"not yours\" and \"not there\" must answer identically"
    );

    // The control that keeps (a) and (b) from being vacuous: the OWNER asking for the SAME asset
    // gets a DIFFERENT answer. Their open clears the ownership gate and travels on down the rail —
    // where it finally stops depends on the build (the rights gate's wallet-link check on a default
    // build; the missing key-authority helper on a dev-rights one), and that is exactly why the
    // assertion is only "not the ownership refusal". So the buyer's 404 above is provably the
    // OWNERSHIP decision, not "this asset cannot be opened by anyone".
    let owner_open = app
        .clone()
        .oneshot(open(Some(&creator.home_token), &creator_uri))
        .await
        .unwrap();
    assert_ne!(
        owner_open.status(),
        StatusCode::NOT_FOUND,
        "the owner's own open must get PAST the ownership gate"
    );
    assert_ne!(
        rail_text(owner_open).await,
        denied_text,
        "the owner must not receive the same refusal a non-owner does"
    );

    // (c) No Home launch at all — refused before any ownership question is even asked.
    let anonymous = app.clone().oneshot(open(None, &creator_uri)).await.unwrap();
    assert_eq!(
        anonymous.status(),
        StatusCode::FORBIDDEN,
        "an unauthenticated open must be refused outright"
    );
}

// ── 3. open → session-bound media → close → sweep ──────────────────────────────

/// STEPS 5–7. Once an acquisition HAS produced a viewer session, that session id in the path is
/// the credential for the media routes — and it is bound three ways: to the viewer capsule, to the
/// owning principal, and to a clock. This proves all three, plus the two ways a session ends:
/// the explicit close (which must kill the read route with it) and the sweeper (which must reap an
/// abandoned one without anybody touching it).
#[tokio::test]
async fn dkms_rail_media_is_session_bound_dies_on_close_and_is_reaped_by_the_sweeper() {
    let dir = tempfile::tempdir().unwrap();
    let buyer = rail_authority(dir.path(), "dkms-rail-viewer-owner-passkey");
    let stranger = rail_authority(dir.path(), "dkms-rail-viewer-stranger-passkey");
    // The player is the viewer capsule an owned-media open launches; a launch token can only be
    // minted for a capsule this node actually has installed.
    let state = test_state(dir.path());
    write_test_capsule_manifest(dir.path(), MEDIA_VIEWER_CAPSULE);
    let app = gateway_router(state);

    let now = crate::auth::now_ts();
    let session = format!("rail-{}", uuid_like_token());
    crate::api::viewer_media::put_media_session(
        session.clone(),
        rail_media_session(&buyer.principal_id, now + 3_600),
    );

    let owner_token = launch_token_for_authority_context(dir.path(), MEDIA_VIEWER_CAPSULE, &buyer);
    let stranger_token =
        launch_token_for_authority_context(dir.path(), MEDIA_VIEWER_CAPSULE, &stranger);
    let manifest_uri = format!("/api/viewers/{MEDIA_VIEWER_CAPSULE}/media/{session}");
    let init_uri = format!("{manifest_uri}/init");

    // (a) The owner streams. The manifest is metadata only; `/init` serves the clear init bytes.
    let manifest = app
        .clone()
        .oneshot(media_get(&manifest_uri, Some(&owner_token)))
        .await
        .unwrap();
    assert_eq!(manifest.status(), StatusCode::OK, "the owner may play");
    let manifest = rail_json(manifest).await;
    assert_eq!(manifest["schema"], json!("elastos.viewer.media/v1"));
    assert_eq!(manifest["segment_count"], json!(3));
    assert_eq!(manifest["is_protected"], json!(true));
    let manifest_text = manifest.to_string();
    for leak in ["cek", "sealed", "private_key", "decrypt_request"] {
        assert!(
            !manifest_text.contains(leak),
            "the play manifest leaked key material ({leak}): {manifest_text}"
        );
    }

    let init = app
        .clone()
        .oneshot(media_get(&init_uri, Some(&owner_token)))
        .await
        .unwrap();
    assert_eq!(init.status(), StatusCode::OK);
    assert_eq!(rail_body(init).await, b"rail-anchor-init-segment");

    // (b) ANOTHER principal, holding a perfectly valid launch token for the SAME viewer capsule,
    //     reads nothing. The session is bearer-bound, and the refusal is the not-found shape — a
    //     stranger cannot even confirm the session id exists.
    let stranger_read = app
        .clone()
        .oneshot(media_get(&init_uri, Some(&stranger_token)))
        .await
        .unwrap();
    assert_eq!(
        stranger_read.status(),
        StatusCode::NOT_FOUND,
        "a media session is bound to its principal and fails closed for anyone else"
    );

    // (c) The right principal with the WRONG capsule's token, and no token at all, both fail.
    //     The credential is scoped to the viewer, not merely to the human.
    let wrong_capsule = app
        .clone()
        .oneshot(media_get(&init_uri, Some(&buyer.home_token)))
        .await
        .unwrap();
    assert_eq!(
        wrong_capsule.status(),
        StatusCode::UNAUTHORIZED,
        "a Home-scoped token is not a media credential"
    );
    let unauthenticated = app
        .clone()
        .oneshot(media_get(&init_uri, None))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    // (d) CLOSE. The explicit release kills the read route with the session, not on a timer.
    let closed = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri(format!("{manifest_uri}/close"))
                .header("x-elastos-home-token", owner_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        closed.status(),
        StatusCode::NO_CONTENT,
        "close is a release"
    );

    let after_close = app
        .clone()
        .oneshot(media_get(&init_uri, Some(&owner_token)))
        .await
        .unwrap();
    assert_eq!(
        after_close.status(),
        StatusCode::NOT_FOUND,
        "the media route dies with the session it was reading"
    );

    // (e) SWEEP. A session nobody closes is reaped on the clock. This one is provably LIVE first
    //     (a real 200 through the read gate, which also runs the store's lazy sweep at `now`), so
    //     when the only subsequent event is the sweeper running with the clock advanced past the
    //     bound, the sweeper is provably the remover — not the next lookup.
    let abandoned = format!("rail-{}", uuid_like_token());
    crate::api::viewer_media::put_media_session(
        abandoned.clone(),
        rail_media_session(&buyer.principal_id, now + 3_600),
    );
    let abandoned_init = format!("/api/viewers/{MEDIA_VIEWER_CAPSULE}/media/{abandoned}/init");
    let live = app
        .clone()
        .oneshot(media_get(&abandoned_init, Some(&owner_token)))
        .await
        .unwrap();
    assert_eq!(
        live.status(),
        StatusCode::OK,
        "the abandoned session is live before the clock passes its bound"
    );

    // Advance the sweeper's clock just past this session's bound and run one pass. The advance is
    // deliberately small (bound + 1s): the store is process-global, and a far-future `now` would
    // reap other tests' sessions rather than only the one under test.
    let released = crate::api::session_lifecycle::sweep_all(now + 3_601);
    assert!(
        released >= 1,
        "the sweeper must report the sessions it released"
    );

    let swept = app
        .clone()
        .oneshot(media_get(&abandoned_init, Some(&owner_token)))
        .await
        .unwrap();
    assert_eq!(
        swept.status(),
        StatusCode::NOT_FOUND,
        "an abandoned session is reaped by the sweeper, with no client close"
    );
}
