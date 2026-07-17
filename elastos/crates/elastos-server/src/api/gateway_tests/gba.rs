use super::*;

#[tokio::test]
async fn home_content_launch_uses_the_bound_gba_viewer_without_compute() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(library_test_state(dir.path()).await);

    let launched = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"gba-ucity"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launched.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launched.into_body(), usize::MAX)
        .await
        .unwrap();
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
        .oneshot(
            Request::builder()
                .uri("/api/viewers/gba-emulator/content/gba-ucity")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
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
