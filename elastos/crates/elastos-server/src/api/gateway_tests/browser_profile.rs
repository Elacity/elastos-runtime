use super::*;
#[cfg(unix)]
use crate::api::browser_profile_reset::{acquire_profile_reset, ProfileResetError};
use crate::api::gateway::gateway_browser::{
    browser_lifecycle_hash, complete_browser_launch, release_browser_page_for_principal,
    reserve_browser_launch, BrowserLaunchEffect, BrowserLaunchLifecycle,
};

#[cfg(unix)]
fn profile_test_disk(root: &std::path::Path) -> std::path::PathBuf {
    root.join("BrowserProfiles/default/profile.ext4")
}

#[cfg(unix)]
fn native_profile_writer_lock(disk: &std::path::Path) -> std::fs::File {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let mut sidecar = disk.as_os_str().to_os_string();
    sidecar.push(".lifetime.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(sidecar)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    file
}

#[cfg(unix)]
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
            browser_instance: None,
            url: "https://example.com/".to_string(),
            exit_id: "local-runtime".to_string(),
            engine_route_provider: "mock-browser-engine".to_string(),
            selected_engine_adapter: Some("mock-adapter".to_string()),
            service_selection: None,
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
            engine_protocol_version: "2.1".to_string(),
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
            viewer_turn_capability: None,
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

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_refuses_native_writer_without_registry_session() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let uri = format!(
        "{}/BrowserProfiles/default/profile.ext4",
        crate::auth::principal_localhost_root(&context.principal_id)
    );
    let disk = rooted_localhost_fs_path(dir.path(), &uri).unwrap();
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"native writer profile state").unwrap();
    let before = std::fs::metadata(&disk).unwrap();
    let writer = native_profile_writer_lock(&disk);
    let token = issue_home_launch_token(dir.path(), BROWSER_CAPSULE_ID).unwrap();
    let response = gateway_router(test_state(dir.path()))
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
    assert_eq!(
        std::fs::read(&disk).unwrap(),
        b"native writer profile state"
    );
    let after = std::fs::metadata(&disk).unwrap();
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    drop(writer);
    assert!(acquire_profile_reset(dir.path().to_path_buf(), disk)
        .await
        .unwrap()
        .remove()
        .await
        .unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_missing_disk_is_idempotent_and_keeps_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let disk = profile_test_disk(dir.path());
    for _ in 0..2 {
        assert!(
            !acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
                .await
                .unwrap()
                .remove()
                .await
                .unwrap()
        );
        assert!(!disk.exists());
        assert!(disk.with_file_name("profile.ext4.lifetime.lock").is_file());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_rejects_unsafe_sidecar_and_disk() {
    use std::os::unix::fs::{symlink, MetadataExt};
    let dir = tempfile::tempdir().unwrap();
    let disk = profile_test_disk(dir.path());
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"saved profile").unwrap();
    let sidecar = disk.with_file_name("profile.ext4.lifetime.lock");
    let outside = dir.path().join("outside");
    std::fs::write(&outside, b"unrelated bytes").unwrap();
    let outside_mode = std::fs::metadata(&outside).unwrap().mode();
    symlink(&outside, &sidecar).unwrap();
    assert!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"unrelated bytes");
    assert_eq!(std::fs::metadata(&outside).unwrap().mode(), outside_mode);
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::hard_link(&outside, &sidecar).unwrap();
    assert!(matches!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone()).await,
        Err(ProfileResetError::Unsafe(_))
    ));
    assert_eq!(std::fs::read(&disk).unwrap(), b"saved profile");
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::create_dir(&sidecar).unwrap();
    assert!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&disk).unwrap(), b"saved profile");
    std::fs::remove_dir(&sidecar).unwrap();

    std::fs::remove_file(&disk).unwrap();
    symlink(&outside, &disk).unwrap();
    assert!(matches!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone()).await,
        Err(ProfileResetError::Unsafe(_))
    ));
    assert_eq!(std::fs::read(&outside).unwrap(), b"unrelated bytes");
    std::fs::remove_file(&disk).unwrap();
    std::fs::create_dir(&disk).unwrap();
    assert!(matches!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone()).await,
        Err(ProfileResetError::Unsafe(_))
    ));
    std::fs::remove_dir(&disk).unwrap();
    std::fs::hard_link(&outside, &disk).unwrap();
    assert!(matches!(
        acquire_profile_reset(dir.path().to_path_buf(), disk).await,
        Err(ProfileResetError::Unsafe(_))
    ));
    assert_eq!(std::fs::read(&outside).unwrap(), b"unrelated bytes");
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_preserves_replaced_disk_and_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let disk = profile_test_disk(dir.path());
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"original profile").unwrap();
    let guard = acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
        .await
        .unwrap();
    let retained = disk.with_file_name("retained.ext4");
    std::fs::rename(&disk, &retained).unwrap();
    std::fs::write(&disk, b"replacement profile").unwrap();
    assert!(matches!(
        guard.remove().await,
        Err(ProfileResetError::Unsafe(_))
    ));
    assert_eq!(std::fs::read(&disk).unwrap(), b"replacement profile");
    assert_eq!(std::fs::read(&retained).unwrap(), b"original profile");

    let guard = acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
        .await
        .unwrap();
    let sidecar = disk.with_file_name("profile.ext4.lifetime.lock");
    std::fs::rename(&sidecar, sidecar.with_file_name("retained.lock")).unwrap();
    std::fs::write(&sidecar, b"replacement lease").unwrap();
    assert!(matches!(
        guard.remove().await,
        Err(ProfileResetError::Unsafe(_))
    ));
    assert_eq!(std::fs::read(&disk).unwrap(), b"replacement profile");
    assert_eq!(std::fs::read(&sidecar).unwrap(), b"replacement lease");
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_cancelled_request_keeps_worker_lease_until_unlink_finishes() {
    use std::os::fd::AsRawFd;
    let dir = tempfile::tempdir().unwrap();
    let disk = profile_test_disk(dir.path());
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"saved profile").unwrap();
    let guard = acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
        .await
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (proceed_tx, proceed_rx) = std::sync::mpsc::channel();
    let removal = tokio::spawn(guard.remove_after_barrier(entered_tx, proceed_rx));
    tokio::task::spawn_blocking(move || {
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    })
    .await
    .unwrap();
    removal.abort();
    assert!(removal.await.unwrap_err().is_cancelled());
    assert!(matches!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone()).await,
        Err(ProfileResetError::Busy)
    ));
    assert_eq!(std::fs::read(&disk).unwrap(), b"saved profile");

    let sidecar = disk.with_file_name("profile.ext4.lifetime.lock");
    let waiter = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&sidecar)
        .unwrap();
    let unlocked = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if unsafe { libc::flock(waiter.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return waiter;
            }
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert!(
                std::time::Instant::now() < deadline,
                "removal worker did not release its lease"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    proceed_tx.send(()).unwrap();
    let waiter = tokio::time::timeout(std::time::Duration::from_secs(5), unlocked)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !disk.exists(),
        "the lease releases only after unlink finishes"
    );
    drop(waiter);
    assert!(!acquire_profile_reset(dir.path().to_path_buf(), disk)
        .await
        .unwrap()
        .remove()
        .await
        .unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_rejects_linked_principal_and_profile_directories() {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let owned = dir.path().join("Users/owned");
    let other = dir.path().join("Users/other");
    let other_disk = profile_test_disk(&other);
    std::fs::create_dir_all(other_disk.parent().unwrap()).unwrap();
    std::fs::write(&other_disk, b"other principal private state").unwrap();
    let other_sidecar = other_disk.with_file_name("profile.ext4.lifetime.lock");
    std::fs::write(&other_sidecar, b"other principal lease").unwrap();
    std::fs::set_permissions(&other_sidecar, std::fs::Permissions::from_mode(0o644)).unwrap();
    let disk_before = std::fs::metadata(&other_disk).unwrap();
    let lease_before = std::fs::metadata(&other_sidecar).unwrap();
    let disk = profile_test_disk(&owned);

    symlink(&other, &owned).unwrap();
    assert!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
            .await
            .is_err()
    );
    std::fs::remove_file(&owned).unwrap();
    std::fs::create_dir(&owned).unwrap();
    symlink(other.join("BrowserProfiles"), owned.join("BrowserProfiles")).unwrap();
    assert!(
        acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
            .await
            .is_err()
    );
    std::fs::remove_file(owned.join("BrowserProfiles")).unwrap();
    std::fs::create_dir(owned.join("BrowserProfiles")).unwrap();
    symlink(other_disk.parent().unwrap(), disk.parent().unwrap()).unwrap();
    assert!(acquire_profile_reset(dir.path().to_path_buf(), disk)
        .await
        .is_err());

    assert_eq!(
        std::fs::read(&other_disk).unwrap(),
        b"other principal private state"
    );
    assert_eq!(
        std::fs::read(&other_sidecar).unwrap(),
        b"other principal lease"
    );
    let disk_after = std::fs::metadata(&other_disk).unwrap();
    let lease_after = std::fs::metadata(&other_sidecar).unwrap();
    assert_eq!(
        (disk_after.dev(), disk_after.ino(), disk_after.mode()),
        (disk_before.dev(), disk_before.ino(), disk_before.mode())
    );
    assert_eq!(
        (lease_after.dev(), lease_after.ino(), lease_after.mode()),
        (lease_before.dev(), lease_before.ino(), lease_before.mode())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_preserves_directory_replacement_after_acquisition() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    let disk = profile_test_disk(dir.path());
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"original principal state").unwrap();
    let original_identity = std::fs::metadata(&disk).unwrap();
    let guard = acquire_profile_reset(dir.path().to_path_buf(), disk.clone())
        .await
        .unwrap();
    let retained_directory = disk.parent().unwrap().with_file_name("retained-default");
    std::fs::rename(disk.parent().unwrap(), &retained_directory).unwrap();
    std::fs::create_dir(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"replacement principal state").unwrap();
    let replacement_identity = std::fs::metadata(&disk).unwrap();

    assert!(matches!(
        guard.remove().await,
        Err(ProfileResetError::Unsafe(_))
    ));
    let retained_disk = retained_directory.join("profile.ext4");
    assert_eq!(
        std::fs::read(&retained_disk).unwrap(),
        b"original principal state"
    );
    assert_eq!(
        std::fs::read(&disk).unwrap(),
        b"replacement principal state"
    );
    let original_after = std::fs::metadata(&retained_disk).unwrap();
    let replacement_after = std::fs::metadata(&disk).unwrap();
    assert_eq!(
        (original_after.dev(), original_after.ino()),
        (original_identity.dev(), original_identity.ino())
    );
    assert_eq!(
        (replacement_after.dev(), replacement_after.ino()),
        (replacement_identity.dev(), replacement_identity.ino())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn browser_profile_reset_allows_os_alias_before_trusted_data_root() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    let alias = dir.path().join("os-alias");
    let root = real.join("runtime");
    let disk = profile_test_disk(&root);
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    std::fs::write(&disk, b"saved profile").unwrap();
    symlink(&real, &alias).unwrap();
    let trusted_root = alias.join("runtime");
    let bound_disk = profile_test_disk(&trusted_root);
    assert!(acquire_profile_reset(trusted_root, bound_disk)
        .await
        .unwrap()
        .remove()
        .await
        .unwrap());
    assert!(!disk.exists());
}
