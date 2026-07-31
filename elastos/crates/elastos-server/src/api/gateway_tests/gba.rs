use super::*;

#[tokio::test]
async fn home_content_launch_uses_the_bound_gba_viewer_without_compute() {
    let dir = tempfile::tempdir().unwrap();
    let state = library_test_state(dir.path()).await;
    write_test_viewer_capsule(
        dir.path(),
        "gba-substitute",
        GBA_EMULATOR_CAPSULE_ID,
        "substitute.gba",
        "Substitute ROM",
    );
    let app = gateway_router(state);

    let launched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"gba-ucity"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = launched.status();
    let body = axum::body::to_bytes(launched.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let launch: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(launch["route"].as_str().is_some_and(
        |route| route.contains("/apps/gba-emulator/") && route.contains("capsule=gba-ucity")
    ));
    assert!(launch["status"].is_null());

    let route = url::Url::parse("http://localhost")
        .unwrap()
        .join(launch["route"].as_str().unwrap())
        .unwrap();
    assert_eq!(route.host_str(), Some("localhost"));
    let token = url::form_urlencoded::parse(route.fragment().unwrap().as_bytes())
        .find_map(|(key, value)| (key == "home_token").then(|| value.into_owned()))
        .unwrap();
    let content = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/content/gba-ucity")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);

    let save_uri = "/api/viewers/gba-emulator/storage/gba-ucity/save/rom-id.sav";
    let saved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(save_uri)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::from("uCity save bytes"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);

    let restored = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(save_uri)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(restored.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"uCity save bytes");

    let viewer_substitution = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/storage/gba-emulator/save/rom-id.sav")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_substitution.status(), StatusCode::UNAUTHORIZED);

    let resource_substitution = app
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/storage/gba-substitute/save/rom-id.sav")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resource_substitution.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn old_install_upgrade_encrypts_ucity_save_and_state_before_normal_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let state = library_test_state(dir.path()).await;
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &context.principal_id);
    let save_uri = format!(
        "{}/.AppData/LocalHost/GBA/ucity/rom-id.sav",
        protection.localhost_root
    );
    let state_uri = format!(
        "{}/.AppData/LocalHost/GBA/ucity/states/rom-id.ss1",
        protection.localhost_root
    );
    let save_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &save_uri).unwrap();
    let state_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &state_uri).unwrap();
    std::fs::create_dir_all(save_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(&save_path, b"old-install uCity save").unwrap();
    std::fs::write(&state_path, b"old-install uCity state").unwrap();

    crate::api::auth_gateway::verify_configured_principal_roots_ready(dir.path())
        .expect_err("old install must not be ready while declared objects are plaintext");
    let backup_parent = dir.path().join("backups");
    std::fs::create_dir_all(&backup_parent).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&backup_parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let backup_dir = backup_parent.join("principal-root-upgrade-regression");
    let receipt = crate::api::auth_gateway::migrate_configured_principal_roots_offline(
        dir.path(),
        &backup_dir,
    )
    .unwrap();

    assert_eq!(receipt.status, "migrated");
    assert_eq!(receipt.root_count, 1);
    assert_eq!(receipt.object_count, 2);
    assert_ne!(
        std::fs::read(&save_path).unwrap(),
        b"old-install uCity save"
    );
    assert_ne!(
        std::fs::read(&state_path).unwrap(),
        b"old-install uCity state"
    );
    assert!(backup_dir.join("upgrade-receipt.json").is_file());
    assert!(backup_dir.join("root-000/receipt.json").is_file());
    let plaintext_backups = std::fs::read_dir(backup_dir.join("root-000"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(".plaintext.backup")
        })
        .collect::<Vec<_>>();
    assert_eq!(plaintext_backups.len(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&backup_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&backup_parent)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(backup_dir.join("upgrade-receipt.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    crate::api::auth_gateway::verify_configured_principal_roots_ready(dir.path()).unwrap();

    let app = gateway_router(state);
    let launched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header(
                    "x-elastos-home-token",
                    issue_home_launch_token_with_context(dir.path(), HOME_CAPSULE_ID, &context)
                        .unwrap(),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"gba-ucity"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = launched.status();
    let body = axum::body::to_bytes(launched.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let launch: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = url::Url::parse("http://localhost")
        .unwrap()
        .join(launch["route"].as_str().unwrap())
        .unwrap();
    let token = url::form_urlencoded::parse(route.fragment().unwrap().as_bytes())
        .find_map(|(key, value)| (key == "home_token").then(|| value.into_owned()))
        .unwrap();

    for (scope, name, before, after) in [
        (
            "save",
            "rom-id.sav",
            b"old-install uCity save".as_slice(),
            b"new-install uCity save".as_slice(),
        ),
        (
            "state",
            "rom-id.ss1",
            b"old-install uCity state".as_slice(),
            b"new-install uCity state".as_slice(),
        ),
    ] {
        let uri = format!("/api/viewers/gba-emulator/storage/gba-ucity/{scope}/{name}");
        let restored = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(HOST, "localhost:61180")
                    .header("origin", "null")
                    .header("x-elastos-home-token", &token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(restored.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], before);

        let saved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(&uri)
                    .header(HOST, "localhost:61180")
                    .header("origin", "null")
                    .header("x-elastos-home-token", &token)
                    .body(Body::from(after.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::NO_CONTENT);

        let reloaded = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(HOST, "localhost:61180")
                    .header("origin", "null")
                    .header("x-elastos-home-token", &token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reloaded.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(reloaded.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], after);
    }
}

#[tokio::test]
async fn selected_gba_content_token_rejects_resource_substitution() {
    let dir = tempfile::tempdir().unwrap();
    let state = library_test_state(dir.path()).await;
    write_test_viewer_capsule(
        dir.path(),
        "gba-substitute",
        GBA_EMULATOR_CAPSULE_ID,
        "substitute.gba",
        "Substitute ROM",
    );
    let app = gateway_router(state);
    let token = issue_home_projection_launch_token_with_context(
        dir.path(),
        "gba-ucity",
        GBA_EMULATOR_CAPSULE_ID,
        &local_home_launch_token_context(dir.path()).unwrap(),
    )
    .unwrap();

    let library = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/library")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(library.status(), StatusCode::OK);
    let body = axum::body::to_bytes(library.into_body(), usize::MAX)
        .await
        .unwrap();
    let library: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = library["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["capsule"], "gba-ucity");

    let selected = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/content/gba-ucity")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);

    let substituted = app
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/content/gba-substitute")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(substituted.status(), StatusCode::UNAUTHORIZED);
    let body = axum::body::to_bytes(substituted.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8(body.to_vec())
        .unwrap()
        .contains("projection authority mismatch"));
}

#[tokio::test]
async fn gba_viewer_reads_only_compatible_library_objects_as_raw_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let context = local_home_launch_token_context(dir.path()).unwrap();
    let principal_root = crate::auth::principal_localhost_root(&context.principal_id);

    for (name, bytes) in [
        ("Library/Test.gba", b"gba bytes".as_slice()),
        ("Library/Test.bin", b"other bytes".as_slice()),
    ] {
        let uri = format!("{principal_root}/{name}");
        let path = elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &uri).unwrap();
        crate::auth::write_principal_root_object(
            dir.path(),
            &context.principal_id,
            &principal_root,
            &uri,
            &path,
            bytes,
        )
        .unwrap();
    }

    let token = issue_home_launch_token(dir.path(), GBA_EMULATOR_CAPSULE_ID).unwrap();
    let gba_uri = url::form_urlencoded::byte_serialize(
        format!("{principal_root}/Library/Test.gba").as_bytes(),
    )
    .collect::<String>();
    let read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/viewers/gba-emulator/library-object?uri={gba_uri}&raw=true"
                ))
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(read.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"gba bytes");

    let unsupported_uri = url::form_urlencoded::byte_serialize(
        format!("{principal_root}/Library/Test.bin").as_bytes(),
    )
    .collect::<String>();
    let denied = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/viewers/gba-emulator/library-object?uri={unsupported_uri}&raw=true"
                ))
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn gba_viewer_save_storage_is_scoped_to_the_launch_principal() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);
    let token = issue_home_launch_token(dir.path(), GBA_EMULATOR_CAPSULE_ID).unwrap();
    let save_uri = "/api/viewers/gba-emulator/storage/gba-emulator/save/rom-id.sav";

    let saved = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(save_uri)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token.clone())
                .body(Body::from("save bytes"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);

    let restored = app
        .oneshot(
            Request::builder()
                .uri(save_uri)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(restored.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"save bytes");
}
