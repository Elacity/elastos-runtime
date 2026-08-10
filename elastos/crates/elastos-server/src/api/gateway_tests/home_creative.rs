//! Contract-era Studio library handlers (P5.1): disk-scan library, playback,
//! delete, stitch validation. Generation itself is the model-provider's job
//! (runs.*) — these tests cover what remains app-side.

use super::*;

fn creative_test_state(dir: &std::path::Path) -> GatewayState {
    GatewayState {
        provider_registry: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.to_path_buf(),
        data_dir: dir.to_path_buf(),
    }
}

fn seed_clip(dir: &std::path::Path, id: &str, prompt: &str) {
    let jobs = dir.join("creative").join("jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    std::fs::write(jobs.join(format!("{id}.mp4")), b"fake-mp4-bytes").unwrap();
    std::fs::write(
        jobs.join(format!("{id}.json")),
        json!({
            "id": id,
            "status": "done",
            "mode": "generate",
            "scale": 2,
            "prompt": prompt,
            "duration": 15.0,
            "sha256": "deadbeef",
            "size": 14
        })
        .to_string(),
    )
    .unwrap();
}

fn home_gui_req(dir: &std::path::Path, method: &str, uri: &str) -> axum::http::Request<Body> {
    let token = issue_home_launch_token(dir, HOME_GUI_SHELL_ID).unwrap();
    let body = if method == "POST" || method == "DELETE" {
        Body::from("{}")
    } else {
        Body::empty()
    };
    test_browser_request("localhost:61180", "null")
        .method(method)
        .uri(uri)
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

#[tokio::test]
async fn creative_library_lists_disk_clips_with_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let clip_id = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
    seed_clip(dir.path(), clip_id, "test clip");
    let app = gateway_router(creative_test_state(dir.path()));

    let response = app
        .oneshot(home_gui_req(dir.path(), "GET", "/api/apps/home/creative/jobs"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let jobs = payload["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["id"], clip_id);
    assert_eq!(jobs[0]["status"], "done");
    assert_eq!(jobs[0]["prompt"], "test clip");
    assert_eq!(jobs[0]["duration"], 15.0);
    assert_eq!(jobs[0]["has_video"], true);
}

#[tokio::test]
async fn creative_library_ignores_non_artifact_files() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = dir.path().join("creative").join("jobs");
    std::fs::create_dir_all(&jobs).unwrap();
    std::fs::write(jobs.join("notes.mp4"), b"x").unwrap(); // not a 32-hex id
    std::fs::write(jobs.join("a1b2c3d4e5f60718293a4b5c6d7e8f90.mp4.partial"), b"x").unwrap();
    let app = gateway_router(creative_test_state(dir.path()));

    let response = app
        .oneshot(home_gui_req(dir.path(), "GET", "/api/apps/home/creative/jobs"))
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["jobs"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn creative_video_streams_clip_and_requires_valid_id() {
    let dir = tempfile::tempdir().unwrap();
    let clip_id = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
    seed_clip(dir.path(), clip_id, "playback");
    let app = gateway_router(creative_test_state(dir.path()));

    let response = app
        .oneshot(home_gui_req(
            dir.path(),
            "GET",
            &format!("/api/apps/home/creative/jobs/{clip_id}/video"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "video/mp4"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"fake-mp4-bytes");

    let app = gateway_router(creative_test_state(dir.path()));
    let bad = app
        .oneshot(home_gui_req(
            dir.path(),
            "GET",
            "/api/apps/home/creative/jobs/not-a-job/video",
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn creative_delete_removes_clip_and_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let clip_id = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
    seed_clip(dir.path(), clip_id, "delete me");
    let app = gateway_router(creative_test_state(dir.path()));

    let response = app
        .oneshot(home_gui_req(
            dir.path(),
            "DELETE",
            &format!("/api/apps/home/creative/jobs/{clip_id}"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!dir
        .path()
        .join("creative/jobs")
        .join(format!("{clip_id}.mp4"))
        .exists());
    assert!(!dir
        .path()
        .join("creative/jobs")
        .join(format!("{clip_id}.json"))
        .exists());

    let app = gateway_router(creative_test_state(dir.path()));
    let again = app
        .oneshot(home_gui_req(
            dir.path(),
            "DELETE",
            &format!("/api/apps/home/creative/jobs/{clip_id}"),
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn creative_stitch_validates_ids_and_existing_clips() {
    let dir = tempfile::tempdir().unwrap();
    let clip_a = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
    let clip_b = "b1b2c3d4e5f60718293a4b5c6d7e8f91";
    seed_clip(dir.path(), clip_a, "shot 1");
    let app = gateway_router(creative_test_state(dir.path()));

    // Too few ids.
    let response = app
        .oneshot({
            let token = issue_home_launch_token(dir.path(), HOME_GUI_SHELL_ID).unwrap();
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/creative/stitch")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "job_ids": [clip_a] }).to_string()))
                .unwrap()
        })
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Second clip missing on disk.
    let app = gateway_router(creative_test_state(dir.path()));
    let response = app
        .oneshot({
            let token = issue_home_launch_token(dir.path(), HOME_GUI_SHELL_ID).unwrap();
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/creative/stitch")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "job_ids": [clip_a, clip_b] }).to_string()))
                .unwrap()
        })
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Bad id shape.
    let app = gateway_router(creative_test_state(dir.path()));
    let response = app
        .oneshot({
            let token = issue_home_launch_token(dir.path(), HOME_GUI_SHELL_ID).unwrap();
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/creative/stitch")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "job_ids": [clip_a, "../../etc/passwd"] }).to_string(),
                ))
                .unwrap()
        })
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn creative_routes_fail_closed_without_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(creative_test_state(dir.path()));
    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("GET")
                .uri("/api/apps/home/creative/jobs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_client_error());
}
