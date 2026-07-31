use super::*;
use crate::api::gateway::gateway_browser::{
    browser_lifecycle_hash, complete_browser_launch, release_browser_page_for_principal,
    reserve_browser_launch, BrowserLaunchEffect, BrowserLaunchLifecycle,
};

#[tokio::test]
async fn browser_profile_reset_removes_only_principal_profile_disk() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let profile_uri = format!(
        "{}/BrowserProfiles/default/profile.ext4",
        crate::auth::principal_localhost_root(&context.principal_id)
    );
    let profile_disk = rooted_localhost_fs_path(dir.path(), &profile_uri).unwrap();
    std::fs::create_dir_all(profile_disk.parent().unwrap()).unwrap();
    std::fs::write(&profile_disk, b"profile-state").unwrap();
    let other_principal_id = "person:local:other";
    let other_profile_uri = format!(
        "{}/BrowserProfiles/default/profile.ext4",
        crate::auth::principal_localhost_root(other_principal_id)
    );
    let other_principal_disk = rooted_localhost_fs_path(dir.path(), &other_profile_uri).unwrap();
    std::fs::create_dir_all(other_principal_disk.parent().unwrap()).unwrap();
    std::fs::write(&other_principal_disk, b"other-principal-state").unwrap();
    let other_disk = dir
        .path()
        .join("legacy-browser-profiles/other-profile.ext4");
    std::fs::create_dir_all(other_disk.parent().unwrap()).unwrap();
    std::fs::write(&other_disk, b"other-state").unwrap();

    let app = gateway_router(test_state(dir.path()));
    let token = issue_home_launch_token(dir.path(), BROWSER_CAPSULE_ID).unwrap();
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/profile/reset")
                .header("x-elastos-home-token", token)
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
    assert_eq!(payload["schema"], "elastos.browser.profile-reset/v1");
    assert_eq!(payload["profile"]["scope"], "active_principal");
    assert_eq!(
        payload["profile"]["storage"],
        "principal_owned_profile_disk"
    );
    assert_eq!(
        payload["profile"]["storage_posture"],
        "principal_owned_reset_scoped_unprotected"
    );
    assert_eq!(payload["profile"]["protected_storage"], false);
    assert_eq!(payload["profile"]["encrypted"], false);
    assert_eq!(payload["profile"]["recoverable"], false);
    assert_eq!(payload["profile"]["recovery"], "not_recovery_kit_packaged");
    assert_eq!(
        payload["profile"]["uri"],
        "localhost://Users/self/BrowserProfiles/default/profile.ext4"
    );
    assert!(payload["profile"].get("profile_key").is_none());
    assert!(payload["profile"].get("principal_id").is_none());
    assert!(payload["profile"].get("disk_path").is_none());
    assert!(!payload.to_string().contains(profile_disk.to_str().unwrap()));
    assert!(!payload
        .to_string()
        .contains(other_principal_disk.to_str().unwrap()));
    assert_eq!(payload["removed_profile_disk"], true);
    assert!(!profile_disk.exists());
    assert!(other_principal_disk.exists());
    assert!(other_disk.exists());
}

#[tokio::test]
async fn browser_profile_reset_requires_browser_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let profile_uri = format!(
        "{}/BrowserProfiles/default/profile.ext4",
        crate::auth::principal_localhost_root(&context.principal_id)
    );
    let profile_disk = rooted_localhost_fs_path(dir.path(), &profile_uri).unwrap();
    std::fs::create_dir_all(profile_disk.parent().unwrap()).unwrap();
    std::fs::write(&profile_disk, b"profile-state").unwrap();

    let app = gateway_router(test_state(dir.path()));
    let missing_token = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/profile/reset")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_token.status(), StatusCode::FORBIDDEN);
    assert!(profile_disk.exists());

    let system_token = issue_home_launch_token(dir.path(), SYSTEM_CAPSULE_ID).unwrap();
    let wrong_app = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/profile/reset")
                .header("x-elastos-home-token", system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_app.status(), StatusCode::FORBIDDEN);
    assert!(profile_disk.exists());
}

#[tokio::test]
async fn browser_profile_reset_refuses_live_principal_session() {
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let profile_uri = format!(
        "{}/BrowserProfiles/default/profile.ext4",
        crate::auth::principal_localhost_root(&context.principal_id)
    );
    let profile_disk = rooted_localhost_fs_path(dir.path(), &profile_uri).unwrap();
    std::fs::create_dir_all(profile_disk.parent().unwrap()).unwrap();
    std::fs::write(&profile_disk, b"profile-state").unwrap();

    let reservation = reserve_browser_launch(
        dir.path(),
        &context.principal_id,
        BrowserLaunchLifecycle {
            owner_launch_id: "launch:profile-reset-test".to_string(),
            url: "https://example.com/".to_string(),
            exit_id: "local-runtime".to_string(),
            engine_route_provider: "mock-browser-engine".to_string(),
            profile_key_hash: browser_lifecycle_hash("profile-test"),
            vm_key_hash: browser_lifecycle_hash("vm-test"),
        },
    )
    .await
    .unwrap();
    complete_browser_launch(
        dir.path(),
        &reservation,
        BrowserLaunchEffect {
            page_id: "profile-reset-live-page".to_string(),
            engine_provider: "browser-engine-adapter".to_string(),
            engine_protocol_version: "2.0".to_string(),
            engine_adapter: "mock-adapter".to_string(),
            engine: "mock-engine".to_string(),
            provider_cleanup: serde_json::json!({
                "schema": "elastos.browser.engine-cleanup-binding/v2",
                "page_id": "profile-reset-live-page",
                "generation": reservation.generation(),
                "stream_id": "stream:profile-reset",
                "adapter": "mock-adapter",
                "engine": "mock-engine",
            }),
            browser_page: serde_json::json!({"page_id": "profile-reset-live-page"}),
            stream_cleanup: None,
        },
    )
    .await
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let token = issue_home_launch_token(dir.path(), BROWSER_CAPSULE_ID).unwrap();
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/profile/reset")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(profile_disk.exists());
    release_browser_page_for_principal(
        dir.path(),
        "profile-reset-live-page",
        &context.principal_id,
        "launch:profile-reset-test",
    )
    .await;
}
