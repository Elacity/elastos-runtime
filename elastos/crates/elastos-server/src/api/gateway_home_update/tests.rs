use super::super::gateway_home_update::{
    check_matches_source, choice_matches, system_runtime_update_summary_with_cache,
    system_update_failure, SystemUpdateApplyRequest, UpdateCheckCache, UpdateCheckKey,
    UpdateCheckSnapshot,
};
use super::home_system::{home_test_get_json, home_test_post_json};
use super::*;

const UPDATE_APPLY_ROUTE: &str = "/api/apps/system/summary";
const UPDATE_APPLY_OPERATION: &str = "system.update.apply";

fn update_intent() -> Value {
    json!({
        "action": "apply", "request_id": "c".repeat(32),
        "source_name": "update-fixture", "channel": "stable",
        "publisher_did": crate::crypto::encode_signing_key_did(
            &ed25519_dalek::SigningKey::from_bytes(&[43; 32])),
        "current_version": "0.7.0", "new_version": "0.7.1",
        "head_cid": TEST_CIDV1, "release_cid": TEST_CIDV0,
    })
}

fn assert_update_stays_unqueued(data_dir: &std::path::Path) {
    for name in ["request.json", "active-request.json", "owner-action.json"] {
        assert!(
            !data_dir.join("update-controller").join(name).exists(),
            "refused System action wrote {name}"
        );
    }
}

fn cached_offer() -> crate::operator_control::OperatorUpdateCheck {
    let intent = update_intent();
    crate::operator_control::OperatorUpdateCheck {
        source_name: intent["source_name"].as_str().unwrap().into(),
        channel: intent["channel"].as_str().unwrap().into(),
        current_version: intent["current_version"].as_str().unwrap().into(),
        latest_version: intent["new_version"].as_str().unwrap().into(),
        update_available: true,
        discovery: "Carrier".into(),
        working_gateway: None,
        head_cid: Some(intent["head_cid"].as_str().unwrap().into()),
        release_cid: Some(intent["release_cid"].as_str().unwrap().into()),
        publisher_did: intent["publisher_did"].as_str().unwrap().into(),
        changes: vec!["Signed update fixture".into()],
    }
}

fn cache_key(policy: &str) -> UpdateCheckKey {
    UpdateCheckKey {
        data_dir: std::path::PathBuf::from("isolated-cache-home"),
        source_policy_sha256: policy.into(),
    }
}

fn unexpected_check() -> std::future::Ready<Option<crate::operator_control::OperatorUpdateCheck>> {
    panic!("summary started a duplicate check")
}

async fn completed_check(
    cache: &mut UpdateCheckCache,
    key: &UpdateCheckKey,
) -> Option<crate::operator_control::OperatorUpdateCheck> {
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            match cache
                .read(key.clone(), std::time::Instant::now(), unexpected_check)
                .await
            {
                UpdateCheckSnapshot::Completed(check) => return check.map(|check| *check),
                UpdateCheckSnapshot::Checking => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("local update worker did not finish")
}

#[tokio::test]
async fn system_update_cache_returns_pending_then_completed_without_duplicate_workers() {
    let mut cache = UpdateCheckCache::default();
    let key = cache_key("policy-a");
    let (release, paused) = tokio::sync::oneshot::channel();
    assert!(matches!(
        cache
            .read(key.clone(), std::time::Instant::now(), || async move {
                paused.await.unwrap()
            })
            .await,
        UpdateCheckSnapshot::Checking
    ));
    for _ in 0..3 {
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            cache.read(key.clone(), std::time::Instant::now(), unexpected_check),
        )
        .await
        .expect("pending worker delayed summary");
        assert!(matches!(result, UpdateCheckSnapshot::Checking));
    }
    release.send(Some(cached_offer())).unwrap();
    let completed = completed_check(&mut cache, &key).await.unwrap();
    assert_eq!(completed.latest_version, "0.7.1");
    let fresh = cache
        .read(
            key.clone(),
            std::time::Instant::now() + std::time::Duration::from_secs(29),
            unexpected_check,
        )
        .await;
    assert!(matches!(fresh, UpdateCheckSnapshot::Completed(Some(_))));
    let (finished, completion) = tokio::sync::oneshot::channel();
    assert!(matches!(
        cache
            .read(
                key.clone(),
                std::time::Instant::now() + std::time::Duration::from_secs(31),
                || async move {
                    finished.send(()).unwrap();
                    None
                },
            )
            .await,
        UpdateCheckSnapshot::Checking
    ));
    // The expiry read advances only its clock; finish the replacement before live-clock reads.
    completion.await.unwrap();
    assert!(completed_check(&mut cache, &key).await.is_none());
}

#[tokio::test]
async fn system_update_cache_refreshes_failed_checks_after_completion_expiry() {
    let mut cache = UpdateCheckCache::default();
    let key = cache_key("policy-a");
    cache
        .read(key.clone(), std::time::Instant::now(), || async { None })
        .await;
    assert!(completed_check(&mut cache, &key).await.is_none());
    assert!(matches!(
        cache
            .read(key.clone(), std::time::Instant::now(), unexpected_check)
            .await,
        UpdateCheckSnapshot::Completed(None)
    ));
    let (release, paused) = tokio::sync::oneshot::channel();
    let (finished, completion) = tokio::sync::oneshot::channel();
    assert!(matches!(
        cache
            .read(
                key.clone(),
                std::time::Instant::now() + std::time::Duration::from_secs(31),
                || async move {
                    let check = paused.await.unwrap();
                    finished.send(()).unwrap();
                    check
                },
            )
            .await,
        UpdateCheckSnapshot::Checking
    ));
    release.send(Some(cached_offer())).unwrap();
    completion.await.unwrap();
    assert!(completed_check(&mut cache, &key).await.is_some());
}

#[tokio::test]
async fn system_update_cache_hides_old_policy_and_home_while_one_worker_finishes() {
    for change_home in [false, true] {
        let mut cache = UpdateCheckCache::default();
        let old_key = cache_key("policy-a");
        let mut new_key = cache_key("policy-b");
        if change_home {
            new_key.source_policy_sha256 = old_key.source_policy_sha256.clone();
            new_key.data_dir = std::path::PathBuf::from("another-isolated-cache-home");
        }
        let (release, paused) = tokio::sync::oneshot::channel();
        cache
            .read(old_key.clone(), std::time::Instant::now(), || async move {
                paused.await.unwrap()
            })
            .await;
        assert!(matches!(
            cache
                .read(new_key.clone(), std::time::Instant::now(), unexpected_check)
                .await,
            UpdateCheckSnapshot::Checking
        ));
        release.send(Some(cached_offer())).unwrap();
        completed_check(&mut cache, &old_key).await.unwrap();
        assert!(matches!(
            cache
                .read(new_key.clone(), std::time::Instant::now(), || async {
                    None
                })
                .await,
            UpdateCheckSnapshot::Checking
        ));
        assert!(completed_check(&mut cache, &new_key).await.is_none());
    }
}

fn configure_update_summary(data_dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let offer = cached_offer();
    crate::sources::save_trusted_sources(
        data_dir,
        &crate::sources::TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".into(),
            default_source: offer.source_name.clone(),
            sources: vec![crate::sources::TrustedSource {
                name: offer.source_name,
                publisher_dids: vec![offer.publisher_did],
                channel: offer.channel,
                discovery_uri: String::new(),
                connect_ticket: String::new(),
                gateways: Vec::new(),
                install_path: data_dir.join("installed-runtime").display().to_string(),
                installed_version: offer.current_version,
                head_cid: String::new(),
                publisher_node_id: String::new(),
                ipns_name: String::new(),
            }],
        },
    )
    .unwrap();
    let directory = data_dir.join("update-controller");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let status = directory.join("status.json");
    std::fs::write(
        &status,
        serde_json::to_vec(&crate::update_controller::UpdateStatus {
            id: None,
            phase: "ready".into(),
            current_version: "0.7.0".into(),
            new_version: None,
            message: "isolated summary fixture".into(),
            controller_pid: std::process::id(),
            controller_start: crate::update_controller::child::process_start(std::process::id())
                .unwrap(),
            host_pid: Some(std::process::id()),
            generation: "a".repeat(32),
        })
        .unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(status, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn system_update_completed_offer_is_bound_to_captured_source_fields_and_signers() {
    let dir = tempfile::tempdir().unwrap();
    configure_update_summary(dir.path());
    let config = crate::sources::load_trusted_sources(dir.path()).unwrap();
    let mut source = config.default_source().unwrap().clone();
    let check = cached_offer();
    assert!(check_matches_source(&source, &check));
    for field in ["source_name", "channel", "current_version", "publisher_did"] {
        let mut changed = serde_json::to_value(&check).unwrap();
        changed[field] = json!("intermediate policy choice");
        let changed = serde_json::from_value(changed).unwrap();
        assert!(!check_matches_source(&source, &changed), "{field}");
    }
    source
        .publisher_dids
        .insert(0, "another trusted publisher".into());
    assert!(check_matches_source(&source, &check));
    source.channel = " stable ".into();
    assert!(check_matches_source(&source, &check));
    source.channel.clear();
    assert!(check_matches_source(&source, &check));
}

#[tokio::test]
async fn system_update_summary_refuses_invalid_installed_version_with_local_repair_hint() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    let context = HomeLaunchTokenContext {
        principal_id: owner.principal_id,
        session_id: owner.session_id,
        proof_binding_id: Some(owner.proof_binding_id),
        grant_id: owner.grant_id,
    };
    configure_update_summary(dir.path());
    let mut sources = crate::sources::load_trusted_sources(dir.path()).unwrap();
    sources.sources[0].installed_version = "broken".into();
    crate::sources::save_trusted_sources(dir.path(), &sources).unwrap();
    let cache = tokio::sync::Mutex::new(UpdateCheckCache::default());
    let summary =
        system_runtime_update_summary_with_cache(dir.path(), &context, &cache, |_, _, _| {
            unexpected_check()
        })
        .await
        .unwrap();
    assert_eq!(summary["available"], false);
    assert_eq!(summary["can_apply"], false);
    assert!(summary["message"]
        .as_str()
        .unwrap()
        .contains("elastos update --force"));
    assert_update_stays_unqueued(dir.path());
    let intent = update_intent();
    let approval = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &owner.system_token,
        UPDATE_APPLY_OPERATION,
        &intent,
    );
    let mut body = intent;
    body["step_up_token"] = json!(approval);
    let app = gateway_router(state);
    let (status, response) =
        home_test_post_json(&app, UPDATE_APPLY_ROUTE, &owner.system_token, "null", body).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("elastos update --force"));
    assert_update_stays_unqueued(dir.path());
}

#[tokio::test]
async fn system_update_summary_stays_fast_while_check_is_pending_and_keeps_queue_eligibility() {
    let dir = tempfile::tempdir().unwrap();
    let _state = test_state(dir.path());
    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    let context = HomeLaunchTokenContext {
        principal_id: owner.principal_id,
        session_id: owner.session_id,
        proof_binding_id: Some(owner.proof_binding_id),
        grant_id: owner.grant_id,
    };
    let cache = tokio::sync::Mutex::new(UpdateCheckCache::default());
    assert!(
        system_runtime_update_summary_with_cache(dir.path(), &context, &cache, |_, _, _| {
            unexpected_check()
        })
        .await
        .is_none()
    );
    configure_update_summary(dir.path());
    let (release, paused) = tokio::sync::oneshot::channel();
    let summary = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        system_runtime_update_summary_with_cache(
            dir.path(),
            &context,
            &cache,
            |_, _, _| async move { paused.await.unwrap() },
        ),
    )
    .await
    .expect("pending update check delayed System summary")
    .unwrap();
    assert_eq!(summary["checking"], true);
    assert_eq!(summary["available"], false);
    assert_eq!(summary["can_apply"], false);
    assert!(summary["new_version"].is_null());
    let public_status = summary["controller"].as_object().unwrap();
    assert_eq!(public_status.len(), 5);
    for field in ["id", "phase", "current_version", "new_version", "message"] {
        assert!(public_status.contains_key(field), "{field}");
    }
    for field in [
        "controller_pid",
        "controller_start",
        "host_pid",
        "generation",
    ] {
        assert!(!public_status.contains_key(field), "{field}");
    }
    release.send(Some(cached_offer())).unwrap();
    let completed = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            let summary = system_runtime_update_summary_with_cache(
                dir.path(),
                &context,
                &cache,
                |_, _, _| unexpected_check(),
            )
            .await
            .unwrap();
            if summary["checking"] == false {
                break summary;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(completed["available"], true);
    assert_eq!(completed["can_apply"], true);
    let member = passkey_authority_with_name_role(
        dir.path(),
        Some("member"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let member_context = HomeLaunchTokenContext {
        principal_id: member.principal_id,
        session_id: member.session_id,
        proof_binding_id: Some(member.proof_binding_id),
        grant_id: member.grant_id,
    };
    let member_summary =
        system_runtime_update_summary_with_cache(dir.path(), &member_context, &cache, |_, _, _| {
            unexpected_check()
        })
        .await;
    assert!(member_summary.is_none());
    std::fs::write(dir.path().join("update-controller/request.json"), b"{}").unwrap();
    let queued =
        system_runtime_update_summary_with_cache(dir.path(), &context, &cache, |_, _, _| {
            unexpected_check()
        })
        .await
        .unwrap();
    assert_eq!(queued["available"], true);
    assert_eq!(queued["can_apply"], false);
}

#[tokio::test]
async fn system_update_guest_summary_skips_discovery_and_keeps_owner_check_unstarted() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let member = passkey_authority_with_name_role(
        dir.path(),
        Some("member"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let context = HomeLaunchTokenContext {
        principal_id: member.principal_id.clone(),
        session_id: member.session_id.clone(),
        proof_binding_id: Some(member.proof_binding_id.clone()),
        grant_id: member.grant_id.clone(),
    };
    configure_update_summary(dir.path());
    let cache = tokio::sync::Mutex::new(UpdateCheckCache::default());
    assert!(
        system_runtime_update_summary_with_cache(dir.path(), &context, &cache, |_, _, _| {
            unexpected_check()
        })
        .await
        .is_none()
    );
    let (status, summary) =
        home_test_get_json(&app, UPDATE_APPLY_ROUTE, &member.system_token, "null").await;
    assert_eq!(status, StatusCode::OK);
    assert!(summary["runtime_update"].is_null());
    assert_update_stays_unqueued(dir.path());

    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    let owner_context = HomeLaunchTokenContext {
        principal_id: owner.principal_id,
        session_id: owner.session_id,
        proof_binding_id: Some(owner.proof_binding_id),
        grant_id: owner.grant_id,
    };
    let (started, observed) = tokio::sync::oneshot::channel();
    let owner_summary = system_runtime_update_summary_with_cache(
        dir.path(),
        &owner_context,
        &cache,
        |_, _, _| async move {
            started.send(()).unwrap();
            Some(cached_offer())
        },
    )
    .await
    .unwrap();
    assert_eq!(owner_summary["checking"], true);
    observed.await.unwrap();
}

#[tokio::test]
async fn system_get_summary_returns_offline_update_state_without_waiting_for_carrier() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    configure_update_summary(dir.path());
    let (status, summary) = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        home_test_get_json(&app, UPDATE_APPLY_ROUTE, &owner.system_token, "null"),
    )
    .await
    .expect("offline update check delayed mandatory System GET");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["runtime_update"]["configured"], true);
    assert_eq!(summary["runtime_update"]["checking"], true);
    assert_eq!(summary["runtime_update"]["can_apply"], false);
    // This source has no transport endpoint. Its retained worker finishes
    // locally, so the fixture leaves no live endpoint or background operation.
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            let (status, summary) =
                home_test_get_json(&app, UPDATE_APPLY_ROUTE, &owner.system_token, "null").await;
            assert_eq!(status, StatusCode::OK);
            if summary["runtime_update"]["checking"] == false {
                assert_eq!(summary["runtime_update"]["available"], false);
                assert_eq!(summary["runtime_update"]["can_apply"], false);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("offline local check did not finish");
}

#[tokio::test]
async fn system_update_post_requires_system_launch_and_admin_role() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    let member = passkey_authority_with_name_role(
        dir.path(),
        Some("member"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let intent = update_intent();
    let approval = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &owner.system_token,
        UPDATE_APPLY_OPERATION,
        &intent,
    );
    let mut body = intent.clone();
    body["step_up_token"] = json!(approval);
    for (token, expected_status, expected_error) in [
        ("", StatusCode::FORBIDDEN, None),
        (owner.home_token.as_str(), StatusCode::FORBIDDEN, None),
        (
            owner.people_token.as_str(),
            StatusCode::UNAUTHORIZED,
            Some("Open System from Home and sign in again."),
        ),
        (
            member.system_token.as_str(),
            StatusCode::FORBIDDEN,
            Some("Ask the Home owner to install this update."),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri(UPDATE_APPLY_ROUTE)
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status);
        let response_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        if let Some(error) = expected_error {
            let payload: Value = serde_json::from_slice(&response_body).unwrap();
            assert_eq!(payload, json!({"error": error}));
        } else {
            // Gateway admission rejects these callers before the System JSON handler.
            assert_eq!(
                response_body.as_ref(),
                b"Gateway request requires an admitted host and caller"
            );
        }
        assert_update_stays_unqueued(dir.path());
    }
}

#[tokio::test]
async fn system_update_post_requires_fresh_exact_passkey_operation() {
    for refusal in ["missing", "expired", "wrong operation", "other principal"] {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));
        let owner = passkey_authority_with_name(dir.path(), Some("owner"));
        let intent = update_intent();
        let approval = match refusal {
            "missing" => String::new(),
            "expired" => stale_step_up_token_for_app_context(
                dir.path(),
                SYSTEM_CAPSULE_ID,
                &owner.system_token,
                UPDATE_APPLY_OPERATION,
                &intent,
            ),
            "wrong operation" => step_up_token_for_app_context(
                dir.path(),
                SYSTEM_CAPSULE_ID,
                &owner.system_token,
                "system.config.save",
                &intent,
            ),
            "other principal" => {
                let other = passkey_authority_with_name_role_credential(
                    dir.path(),
                    Some("other owner"),
                    crate::auth::RuntimePrincipalRole::Admin,
                    "update-other-owner-passkey",
                );
                step_up_token_for_app_context(
                    dir.path(),
                    SYSTEM_CAPSULE_ID,
                    &other.system_token,
                    UPDATE_APPLY_OPERATION,
                    &intent,
                )
            }
            _ => unreachable!(),
        };
        let mut body = intent;
        body["step_up_token"] = json!(approval);
        let (status, _) =
            home_test_post_json(&app, UPDATE_APPLY_ROUTE, &owner.system_token, "null", body).await;
        assert!(status.is_client_error(), "{refusal}: {status}");
        assert_update_stays_unqueued(dir.path());
    }
}

#[tokio::test]
async fn system_update_post_binds_every_approved_intent_field() {
    for field in [
        "action",
        "request_id",
        "source_name",
        "channel",
        "publisher_did",
        "current_version",
        "new_version",
        "head_cid",
        "release_cid",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));
        let owner = passkey_authority_with_name(dir.path(), Some("owner"));
        let intent = update_intent();
        let approval = step_up_token_for_app_context(
            dir.path(),
            SYSTEM_CAPSULE_ID,
            &owner.system_token,
            UPDATE_APPLY_OPERATION,
            &intent,
        );
        let mut body = intent;
        body[field] = json!(if field == "request_id" {
            "d".repeat(32)
        } else {
            "changed approved intent".to_owned()
        });
        body["step_up_token"] = json!(approval);
        let (status, _) =
            home_test_post_json(&app, UPDATE_APPLY_ROUTE, &owner.system_token, "null", body).await;
        assert!(status.is_client_error(), "{field}: {status}");
        assert_update_stays_unqueued(dir.path());
    }
}

#[tokio::test]
async fn system_update_recovered_approval_keeps_its_preconsumption_intent() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let owner = passkey_authority_with_name(dir.path(), Some("owner"));
    let intent = update_intent();
    let approval = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &owner.system_token,
        UPDATE_APPLY_OPERATION,
        &intent,
    );
    let mut body = intent;
    body["step_up_token"] = json!(approval);
    // Metadata fails in this isolated Home. Approval is reserved before consumption,
    // so the exact retry can recheck the same intent without another passkey.
    for attempt in 0..2 {
        let (status, _) = home_test_post_json(
            &app,
            UPDATE_APPLY_ROUTE,
            &owner.system_token,
            "null",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "attempt {attempt}");
        for name in ["request.json", "active-request.json"] {
            assert!(!dir.path().join("update-controller").join(name).exists());
        }
        let receipt: Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("update-controller/owner-action.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["queued"], false);
        assert_eq!(receipt["request"]["id"], "c".repeat(32));
    }
}

#[test]
fn system_update_choice_matches_every_signed_offer_field() {
    let intent = update_intent();
    let request = crate::update_controller::UpdateRequest {
        id: intent["request_id"].as_str().unwrap().into(),
        source_name: intent["source_name"].as_str().unwrap().into(),
        channel: intent["channel"].as_str().unwrap().into(),
        publisher_did: intent["publisher_did"].as_str().unwrap().into(),
        current_version: intent["current_version"].as_str().unwrap().into(),
        new_version: intent["new_version"].as_str().unwrap().into(),
        head_cid: intent["head_cid"].as_str().unwrap().into(),
        release_cid: intent["release_cid"].as_str().unwrap().into(),
    };
    let check = crate::operator_control::OperatorUpdateCheck {
        source_name: request.source_name.clone(),
        channel: request.channel.clone(),
        current_version: request.current_version.clone(),
        latest_version: request.new_version.clone(),
        update_available: true,
        discovery: "Carrier".into(),
        working_gateway: None,
        head_cid: Some(request.head_cid.clone()),
        release_cid: Some(request.release_cid.clone()),
        publisher_did: request.publisher_did.clone(),
        changes: Vec::new(),
    };
    assert!(choice_matches(&request, &check));
    for field in [
        "source_name",
        "channel",
        "publisher_did",
        "current_version",
        "new_version",
        "head_cid",
        "release_cid",
    ] {
        let mut changed = serde_json::to_value(&request).unwrap();
        changed[field] = json!("different signed choice");
        let changed = serde_json::from_value(changed).unwrap();
        assert!(!choice_matches(&changed, &check), "{field}");
    }
    let mut unavailable = check.clone();
    unavailable.update_available = false;
    assert!(!choice_matches(&request, &unavailable));
    unavailable = check.clone();
    unavailable.head_cid = None;
    assert!(!choice_matches(&request, &unavailable));
    unavailable = check;
    unavailable.release_cid = None;
    assert!(!choice_matches(&request, &unavailable));
}

#[test]
fn system_update_post_rejects_hidden_authority_fields() {
    for field in [
        "admin",
        "owner",
        "auto_confirm",
        "recovered",
        "effect_id",
        "request_sha256",
    ] {
        let mut body = update_intent();
        body["step_up_token"] = json!("fixture token");
        body[field] = json!(true);
        assert!(
            serde_json::from_value::<SystemUpdateApplyRequest>(body).is_err(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn system_update_exact_retry_recovers_both_queue_write_boundaries() {
    for dispatched in ["queued", "active", "completed"] {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);
        let owner = passkey_authority_with_name(dir.path(), Some("owner"));
        let intent = update_intent();
        let approval = step_up_token_for_app_context(
            dir.path(),
            SYSTEM_CAPSULE_ID,
            &owner.system_token,
            UPDATE_APPLY_OPERATION,
            &intent,
        );
        let mut body = intent.clone();
        body["step_up_token"] = json!(approval);
        let request = crate::update_controller::UpdateRequest {
            id: intent["request_id"].as_str().unwrap().into(),
            source_name: intent["source_name"].as_str().unwrap().into(),
            channel: intent["channel"].as_str().unwrap().into(),
            publisher_did: intent["publisher_did"].as_str().unwrap().into(),
            current_version: intent["current_version"].as_str().unwrap().into(),
            new_version: intent["new_version"].as_str().unwrap().into(),
            head_cid: intent["head_cid"].as_str().unwrap().into(),
            release_cid: intent["release_cid"].as_str().unwrap().into(),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-elastos-home-token",
            HeaderValue::from_str(&owner.system_token).unwrap(),
        );
        // Use the same native null-origin System launch binding as the router fixture.
        headers.insert("Origin", HeaderValue::from_static("null"));
        headers.insert(HOST, HeaderValue::from_static("localhost:61180"));
        let launch =
            require_home_launch_token_binding(dir.path(), &headers, &[SYSTEM_CAPSULE_ID]).unwrap();
        consume_prepared_passkey_step_up_effect(
            dir.path(),
            body["step_up_token"].as_str().unwrap(),
            &launch,
            180,
            UPDATE_APPLY_OPERATION,
            &intent,
            |effect| {
                crate::update_controller::reserve_owner_update(
                    dir.path(),
                    &request,
                    &effect.step_up_id,
                    &effect.request_sha256,
                    effect.recovered,
                )
            },
        )
        .unwrap();
        let controller = dir.path().join("update-controller");
        let (name, bytes) = if dispatched == "completed" {
            (
                "status.json",
                serde_json::to_vec(&crate::update_controller::UpdateStatus {
                    id: Some(request.id.clone()),
                    phase: "updated".into(),
                    current_version: request.new_version.clone(),
                    new_version: Some(request.new_version.clone()),
                    message: "completed fixture".into(),
                    controller_pid: std::process::id(),
                    controller_start: crate::update_controller::child::process_start(
                        std::process::id(),
                    )
                    .unwrap(),
                    host_pid: Some(std::process::id()),
                    generation: "a".repeat(32),
                })
                .unwrap(),
            )
        } else {
            (
                if dispatched == "queued" {
                    "request.json"
                } else {
                    "active-request.json"
                },
                serde_json::to_vec(&request).unwrap(),
            )
        };
        let path = controller.join(name);
        std::fs::write(&path, &bytes).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        for _ in 0..2 {
            let (status, result) = home_test_post_json(
                &app,
                UPDATE_APPLY_ROUTE,
                &owner.system_token,
                "null",
                body.clone(),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{dispatched}: {result}");
            assert_eq!(result["id"], request.id);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(controller.join("owner-action.json")).unwrap())
                .unwrap();
        assert_eq!(receipt["queued"], true);
        if dispatched == "completed" {
            assert!(!controller.join("request.json").exists());
        }
    }
}

#[tokio::test]
async fn system_update_failures_explain_the_cause_without_private_error_details() {
    for (error, expected_status, expected_cause) in [
        (
            anyhow::Error::from(crate::update::UpdateSourceUnavailable)
                .context("private source endpoint"),
            StatusCode::FAILED_DEPENDENCY,
            "Connect to the internet and select Update again.",
        ),
        (
            anyhow::Error::from(crate::update::InvalidInstalledVersion)
                .context("private source detail"),
            StatusCode::CONFLICT,
            "elastos update --force",
        ),
        (
            anyhow::anyhow!("private verification detail"),
            StatusCode::CONFLICT,
            "release was refused or verification failed",
        ),
    ] {
        let response = system_update_failure(&error);
        assert_eq!(response.status(), expected_status);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let message = value["error"].as_str().unwrap();
        assert!(message.contains(expected_cause));
        assert!(!message.contains("private"));
    }
}
