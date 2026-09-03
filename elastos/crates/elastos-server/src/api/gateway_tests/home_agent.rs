use super::*;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use serde_json::Value;

async fn response_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn get_workspace(token: String) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .method("GET")
        .uri("/api/apps/home-agent/workspace")
        .header("x-elastos-home-token", token)
        .body(Body::empty())
        .unwrap()
}

fn put_workspace(token: String, body: Value) -> Request<Body> {
    test_browser_request("localhost:61180", "null")
        .method("PUT")
        .uri("/api/apps/home-agent/workspace")
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn workspace_object(
    data_dir: &std::path::Path,
    principal_id: &str,
) -> (String, std::path::PathBuf) {
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let object_uri = format!("{localhost_root}/.AppData/ElastOS/HomeAgent/workspace.json");
    let object_path =
        elastos_common::localhost::rooted_localhost_fs_path(data_dir, &object_uri).unwrap();
    (object_uri, object_path)
}

fn sample_put(if_revision: u64) -> Value {
    json!({
        "schema": "elastos.home-agent.workspace/v1",
        "if_revision": if_revision,
        "document": {
            "v": 7,
            "activeSessionId": "s-1",
            "sessions": [
                { "id": "s-1", "title": "First chat", "messages": [
                    { "role": "user", "text": "hello there" },
                    { "role": "agent", "text": "hi" }
                ] }
            ],
            "composerDraft": "Draft note"
        }
    })
}

#[tokio::test]
async fn home_agent_workspace_absent_get_is_read_only_and_returns_revision_zero() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("home-agent-user"));
    let token = app_token_for_authority(dir.path(), "home-agent", &authority);
    let (_, object_path) = workspace_object(dir.path(), &authority.principal_id);
    assert!(!object_path.exists());

    let response = app.oneshot(get_workspace(token)).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "schema": "elastos.home-agent.workspace/v1",
            "revision": 0,
            "document": {}
        })
    );
    assert!(!object_path.exists());
}

#[tokio::test]
async fn home_agent_workspace_round_trip_and_restart_preserve_exact_document() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("home-agent-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "home-agent", &authority);
    let app = gateway_router(test_state(dir.path()));

    let stored = app
        .clone()
        .oneshot(put_workspace(token.clone(), sample_put(0)))
        .await
        .unwrap();
    assert_eq!(stored.status(), StatusCode::OK);
    let stored_json = response_json(stored).await;
    assert_eq!(stored_json["revision"], 1);
    assert_eq!(stored_json["document"], sample_put(0)["document"]);

    let restarted = gateway_router(test_state(dir.path()));
    let loaded = restarted.oneshot(get_workspace(token)).await.unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    assert_eq!(response_json(loaded).await, stored_json);
}

#[tokio::test]
async fn home_agent_workspace_wrong_capsule_is_forbidden_and_principals_stay_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("home-agent-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "home-agent", &authority);
    let app = gateway_router(test_state(dir.path()));

    let written = app
        .clone()
        .oneshot(put_workspace(token.clone(), sample_put(0)))
        .await
        .unwrap();
    assert_eq!(written.status(), StatusCode::OK);

    // Home's own token is not the capsule's launch token.
    let forbidden = app
        .clone()
        .oneshot(get_workspace(authority.system_token.clone()))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // The Assistant's launch token does not open the Home Agent's object.
    let assistant_token = app_token_for_authority(dir.path(), "assistant", &authority);
    let forbidden = app
        .clone()
        .oneshot(get_workspace(assistant_token))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // Another principal sees an empty workspace, never this one's.
    let other = passkey_authority_with_name_role(
        dir.path(),
        Some("guest-user"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let other_token = app_token_for_authority(dir.path(), "home-agent", &other);
    let other_view = app.oneshot(get_workspace(other_token)).await.unwrap();
    assert_eq!(other_view.status(), StatusCode::OK);
    assert_eq!(response_json(other_view).await["revision"], 0);
}

#[tokio::test]
async fn home_agent_workspace_stale_and_future_revisions_fail_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("home-agent-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "home-agent", &authority);
    let app = gateway_router(test_state(dir.path()));

    let initial = app
        .clone()
        .oneshot(put_workspace(token.clone(), sample_put(0)))
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);

    for revision in [0_u64, 2_u64] {
        let stale = app
            .clone()
            .oneshot(put_workspace(token.clone(), sample_put(revision)))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
    }

    let loaded = app.oneshot(get_workspace(token)).await.unwrap();
    let loaded_json = response_json(loaded).await;
    assert_eq!(loaded_json["revision"], 1);
    assert_eq!(loaded_json["document"]["composerDraft"], "Draft note");
}

#[tokio::test]
async fn home_agent_workspace_invalid_requests_fail_closed_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("home-agent-user"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "home-agent", &authority);
    let app = gateway_router(test_state(dir.path()));
    let (_, object_path) = workspace_object(dir.path(), &authority.principal_id);

    let mut too_deep = json!(1);
    for _ in 0..40 {
        too_deep = json!({ "n": too_deep });
    }
    let invalid = [
        json!({ "schema": "elastos.assistant.workspace/v1", "if_revision": 0, "document": {} }),
        json!({ "schema": "elastos.home-agent.workspace/v1", "if_revision": 0, "document": [] }),
        json!({ "schema": "elastos.home-agent.workspace/v1", "if_revision": 0, "document": "x" }),
        json!({ "schema": "elastos.home-agent.workspace/v1", "if_revision": 0, "document": {}, "extra": 1 }),
        json!({ "schema": "elastos.home-agent.workspace/v1", "if_revision": 0, "document": too_deep }),
    ];
    for body in invalid {
        let response = app
            .clone()
            .oneshot(put_workspace(token.clone(), body))
            .await
            .unwrap();
        assert!(
            response.status() == StatusCode::BAD_REQUEST
                || response.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "unexpected status {}",
            response.status()
        );
        assert!(!object_path.exists());
    }

    let oversized = json!({
        "schema": "elastos.home-agent.workspace/v1",
        "if_revision": 0,
        "document": { "blob": "x".repeat(1024 * 1024 + 1) }
    });
    let response = app
        .clone()
        .oneshot(put_workspace(token.clone(), oversized))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(!object_path.exists());
}

#[tokio::test]
async fn home_agent_workspace_is_declared_for_recovery_and_written_as_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("home-agent-user"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let token = app_token_for_authority(dir.path(), "home-agent", &authority);
    let app = gateway_router(test_state(dir.path()));
    let (object_uri, object_path) = workspace_object(dir.path(), &authority.principal_id);

    let response = app
        .oneshot(put_workspace(token, sample_put(0)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let inventory = crate::api::auth_gateway::principal_root_protected_object_inventory(
        dir.path(),
        &protection.localhost_root,
    );
    assert!(inventory
        .iter()
        .map(crate::auth::PrincipalRootProtectedObjectDeclarationV1::uri)
        .any(|uri| uri.ends_with("/.AppData/ElastOS/HomeAgent")));

    let ciphertext = std::fs::read(&object_path).unwrap();
    let raw_text = String::from_utf8_lossy(&ciphertext);
    assert!(!raw_text.contains("hello there"));
    assert!(!raw_text.contains("elastos.home-agent.workspace/v1"));

    let decrypted = crate::auth::read_principal_root_object(
        dir.path(),
        &authority.principal_id,
        &protection.localhost_root,
        &object_uri,
        &object_path,
    )
    .unwrap();
    let workspace: Value = serde_json::from_slice(&decrypted).unwrap();
    assert_eq!(workspace["revision"], 1);
    assert_eq!(workspace["document"]["sessions"][0]["title"], "First chat");
}
