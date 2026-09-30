use super::*;

// Large preserved workspaces should fail with bounded diagnostics, not a dump
// of every conversation. Equality remains exact for the original values.
macro_rules! assert_bounded_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let (left, right) = (&$left, &$right);
        assert!(
            left == right,
            "equality failed: left={} right={}",
            format!("{left:?}").chars().take(180).collect::<String>(),
            format!("{right:?}").chars().take(180).collect::<String>()
        );
    }};
}

const V2: &str = "/api/apps/assistant/workspace-v2";
const V1: &str = "/api/apps/assistant/workspace";
const AGENT: &str = "/api/apps/home-agent/workspace";
const HOME: &str = "/api/apps/home/state";
const NEW_OBJECT: &str = ".AppData/ElastOS/Assistant/workspace-v2.json";

struct Fixture {
    dir: tempfile::TempDir,
    principal: String,
    assistant: String,
    agent: String,
    shell: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let authority = passkey_authority_with_name(dir.path(), Some("workspace-migration"));
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
        Self {
            principal: authority.principal_id.clone(),
            assistant: app_token_for_authority(dir.path(), "assistant", &authority),
            agent: app_token_for_authority(dir.path(), "home-agent", &authority),
            shell: app_token_for_authority(dir.path(), "home-gui", &authority),
            dir,
        }
    }

    fn app(&self) -> axum::Router {
        gateway_router(test_state(self.dir.path()))
    }

    fn location(&self, relative: &str) -> (String, std::path::PathBuf) {
        let root = crate::auth::principal_localhost_root(&self.principal);
        let uri = format!("{root}/{relative}");
        let path =
            elastos_common::localhost::rooted_localhost_fs_path(self.dir.path(), &uri).unwrap();
        (uri, path)
    }

    fn write(&self, relative: &str, value: &Value) {
        let (uri, path) = self.location(relative);
        crate::auth::write_protected_principal_root_object(
            self.dir.path(),
            &self.principal,
            &crate::auth::principal_localhost_root(&self.principal),
            &uri,
            &path,
            &serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    fn read(&self, relative: &str) -> Value {
        let (uri, path) = self.location(relative);
        let bytes = crate::auth::read_principal_root_object(
            self.dir.path(),
            &self.principal,
            &crate::auth::principal_localhost_root(&self.principal),
            &uri,
            &path,
        )
        .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn seed(&self) -> Value {
        let run = format!("run:sha256:{}", "a".repeat(64));
        let assistant = json!({ "schema": "elastos.assistant.workspace/v1", "revision": 7,
            "sessions": [{ "id": "same-id", "title": "A".repeat(150), "mode": "build", "pinned": true,
                "messages": (0..32).map(|_| json!({"role":"tool", "content":"x".repeat(5000), "run_id":run})).collect::<Vec<_>>() }],
            "draft": "legacy assistant draft", "selected_offer_id": "offer:legacy" });
        let agent = json!({ "schema": "elastos.home-agent.workspace/v1", "revision": 9, "document": {
            "v":1, "activeSessionId":"same-id", "liveOfferId":"offer:qwen", "selectedModelCid":TEST_CIDV1,
            "sessions":[{"id":"same-id", "title":"Qwen", "messages":[{"role":"agent", "text":"Qwen is ready.",
                "turn":{"providerRunId":run, "createRequestId":"request:exact", "state":"settlement_unknown"}}]}],
            "composerDraft":{"text":"keep draft", "parts":[{"uri":"localhost://draft", "text":"keep attachment"}]},
            "opaqueFutureField":{"preserve":true} } });
        let legacy = json!({ "v":1, "sessions":[{"id":"same-id", "messages":[{"role":"agent", "text":"legacy shell reply"}]}], "composerDraft":{"text":"third draft"} });
        self.write(".AppData/ElastOS/Assistant/workspace.json", &assistant);
        self.write(".AppData/ElastOS/HomeAgent/workspace.json", &agent);
        self.write(
            ".AppData/ElastOS/Home/browser-state.json",
            &json!({
                "schema":"elastos.home.browser-state/v1", "principal_id":self.principal,
                "localhost_root":crate::auth::principal_localhost_root(&self.principal),
                "session":{"windows":[], "agent":legacy}, "layout":null, "recent_targets":[]
            }),
        );
        json!({"assistant":assistant,"homeAgent":agent,"homeSessionAgent":legacy})
    }
}

fn request(endpoint: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let method = if body.is_some() {
        if endpoint == HOME {
            "POST"
        } else {
            "PUT"
        }
    } else {
        "GET"
    };
    test_browser_request("localhost:61180", "null")
        .method(method)
        .uri(endpoint)
        .header("x-elastos-home-token", token)
        .header(CONTENT_TYPE, "application/json")
        .body(
            body.map(|value| Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap()
}

async fn json_body(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn adopt(get: &Value, document: Value) -> Value {
    json!({"schema":"elastos.assistant.workspace/v2", "if_revision":get["revision"], "migration_revision":get["migration_revision"], "document":document})
}

#[tokio::test]
async fn assistant_v2_read_is_side_effect_free_and_captures_all_three_sources_exactly() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let absent = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    assert_bounded_eq!(absent["revision"], 0);
    assert_bounded_eq!(
        absent["legacy"],
        json!({"assistant":null,"homeAgent":null,"homeSessionAgent":null})
    );
    assert!(!fixture.location(NEW_OBJECT).1.exists());
    let sources = fixture.seed();
    let response = app
        .oneshot(request(V2, &fixture.assistant, None))
        .await
        .unwrap();
    assert_bounded_eq!(response.status(), StatusCode::OK);
    let loaded = json_body(response).await;
    assert_bounded_eq!(loaded["legacy"], sources);
    assert_ne!(loaded["migration_revision"], absent["migration_revision"]);
    assert!(!fixture.location(NEW_OBJECT).1.exists());
}

#[tokio::test]
async fn assistant_v2_adoption_retains_sources_ciphertext_and_exact_document_across_restart() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let sources = fixture.seed();
    let originals = [
        ".AppData/ElastOS/Assistant/workspace.json",
        ".AppData/ElastOS/HomeAgent/workspace.json",
        ".AppData/ElastOS/Home/browser-state.json",
    ];
    let before: Vec<_> = originals
        .iter()
        .map(|p| std::fs::read(fixture.location(p).1).unwrap())
        .collect();
    let initial = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    let document =
        json!({"v":1,"sources":sources,"sessions":[{"id":"same-id","unknown":{"intact":true}}]});
    let response = app
        .clone()
        .oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&initial, document.clone())),
        ))
        .await
        .unwrap();
    assert_bounded_eq!(response.status(), StatusCode::OK);
    let adopted = json_body(response).await;
    assert_bounded_eq!(
        adopted,
        json!({"schema":"elastos.assistant.workspace/v2","revision":1,"document":document})
    );
    assert_bounded_eq!(fixture.read(NEW_OBJECT)["legacy"], sources);
    let restarted = fixture.app();
    assert_bounded_eq!(
        json_body(
            restarted
                .oneshot(request(V2, &fixture.assistant, None))
                .await
                .unwrap()
        )
        .await,
        adopted
    );
    for (index, path) in originals.iter().enumerate() {
        assert_bounded_eq!(
            std::fs::read(fixture.location(path).1).unwrap(),
            before[index]
        );
    }
    let raw = std::fs::read(fixture.location(NEW_OBJECT).1).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("legacy assistant draft"));
    let saved = app.oneshot(request(V2, &fixture.assistant, Some(json!({"schema":"elastos.assistant.workspace/v2","if_revision":1,"document":{"v":1}})))).await.unwrap();
    assert_bounded_eq!(saved.status(), StatusCode::OK);
    assert_bounded_eq!(fixture.read(NEW_OBJECT)["legacy"], sources);
    assert_bounded_eq!(fixture.read(NEW_OBJECT)["revision"], 2);
}

#[tokio::test]
async fn assistant_v2_source_change_and_concurrent_adoption_conflict_without_overwrite() {
    let fixture = Fixture::new();
    let app = fixture.app();
    fixture.seed();
    let initial = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    let old_write = json!({"schema":"elastos.home-agent.workspace/v1","if_revision":9,"document":{"v":1,"composerDraft":{"text":"concurrent draft"}}});
    assert_bounded_eq!(
        app.clone()
            .oneshot(request(AGENT, &fixture.agent, Some(old_write)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let stale = app
        .clone()
        .oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&initial, json!({}))),
        ))
        .await
        .unwrap();
    assert_bounded_eq!(stale.status(), StatusCode::CONFLICT);
    assert_bounded_eq!(
        json_body(stale).await["code"],
        "assistant_workspace_migration_conflict"
    );
    assert!(!fixture.location(NEW_OBJECT).1.exists());
    let fresh = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    let (left, right) = tokio::join!(
        app.clone().oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&fresh, json!({"owner":"left"})))
        )),
        app.clone().oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&fresh, json!({"owner":"right"})))
        )),
    );
    let mut statuses = [
        left.unwrap().status().as_u16(),
        right.unwrap().status().as_u16(),
    ];
    statuses.sort();
    assert_bounded_eq!(statuses, [200, 409]);
    let before = std::fs::read(fixture.location(NEW_OBJECT).1).unwrap();
    for revision in [0, 2] {
        let result = app.clone().oneshot(request(V2, &fixture.assistant, Some(json!({"schema":"elastos.assistant.workspace/v2","if_revision":revision,"document":{}})))).await.unwrap();
        assert_bounded_eq!(result.status(), StatusCode::CONFLICT);
    }
    assert_bounded_eq!(
        std::fs::read(fixture.location(NEW_OBJECT).1).unwrap(),
        before
    );
}

#[tokio::test]
async fn assistant_v2_fences_old_writers_and_preserves_legacy_agent_on_window_updates() {
    let fixture = Fixture::new();
    let app = fixture.app();
    let sources = fixture.seed();
    let before_windows = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    // A window-only update preserves the legacy Agent even before adoption.
    let window_update = json!({"session":{"windows":[],"desktops":["work"]}});
    assert_bounded_eq!(
        app.clone()
            .oneshot(request(HOME, &fixture.shell, Some(window_update.clone())))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_bounded_eq!(
        fixture.read(".AppData/ElastOS/Home/browser-state.json")["session"]["agent"],
        sources["homeSessionAgent"]
    );
    let initial = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    assert_bounded_eq!(
        initial["migration_revision"],
        before_windows["migration_revision"]
    );
    assert_bounded_eq!(
        app.clone()
            .oneshot(request(
                V2,
                &fixture.assistant,
                Some(adopt(&initial, json!({"v":1})))
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for (endpoint, token, body) in [
        (
            V1,
            &fixture.assistant,
            json!({"schema":"elastos.assistant.workspace/v1","if_revision":7,"sessions":[],"draft":"old client"}),
        ),
        (
            AGENT,
            &fixture.agent,
            json!({"schema":"elastos.home-agent.workspace/v1","if_revision":9,"document":{}}),
        ),
        (
            HOME,
            &fixture.shell,
            json!({"session":{"windows":[],"agent":{"v":1,"changed":true}}}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(endpoint, token, Some(body)))
            .await
            .unwrap();
        assert_bounded_eq!(response.status(), StatusCode::CONFLICT);
        assert_bounded_eq!(
            json_body(response).await["code"],
            "assistant_workspace_migrated"
        );
    }
    assert_bounded_eq!(
        app.clone()
            .oneshot(request(HOME, &fixture.shell, Some(window_update)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_bounded_eq!(
        fixture.read(".AppData/ElastOS/Home/browser-state.json")["session"]["agent"],
        sources["homeSessionAgent"]
    );
    assert_bounded_eq!(
        fixture.read(".AppData/ElastOS/Assistant/workspace.json"),
        sources["assistant"]
    );
    assert_bounded_eq!(
        fixture.read(".AppData/ElastOS/HomeAgent/workspace.json"),
        sources["homeAgent"]
    );
}

#[tokio::test]
async fn assistant_v2_rejects_wrong_capsules_other_principals_and_malformed_sources() {
    let fixture = Fixture::new();
    let app = fixture.app();
    fixture.seed();
    for token in [&fixture.agent, &fixture.shell, ""] {
        assert!(matches!(
            app.clone()
                .oneshot(request(V2, token, None))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
        ));
        assert!(matches!(app.clone().oneshot(request(V2, token, Some(json!({"schema":"elastos.assistant.workspace/v2","if_revision":0,"document":{}})))).await.unwrap().status(), StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED));
    }
    let mut wrong_origin = request(V2, &fixture.assistant, None);
    wrong_origin.headers_mut().insert(
        "origin",
        HeaderValue::from_static("https://unrelated.invalid"),
    );
    assert_bounded_eq!(
        app.clone().oneshot(wrong_origin).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let other = passkey_authority_with_name_role_credential(
        fixture.dir.path(),
        Some("another-owner"),
        crate::auth::RuntimePrincipalRole::Admin,
        "assistant-v2-isolation-other-passkey",
    );
    assert_ne!(
        fixture.principal, other.principal_id,
        "isolation fixture must use distinct principals"
    );
    let token = app_token_for_authority(fixture.dir.path(), "assistant", &other);
    let empty = json_body(
        app.clone()
            .oneshot(request(V2, &token, None))
            .await
            .unwrap(),
    )
    .await;
    assert_bounded_eq!(
        empty["legacy"],
        json!({"assistant":null,"homeAgent":null,"homeSessionAgent":null})
    );
    for (relative, bad) in [
        (
            ".AppData/ElastOS/Assistant/workspace.json",
            json!({"schema":"unknown"}),
        ),
        (
            ".AppData/ElastOS/HomeAgent/workspace.json",
            json!({"schema":"elastos.home-agent.workspace/v1","revision":1,"document":[]}),
        ),
        (
            ".AppData/ElastOS/Home/browser-state.json",
            json!({"schema":"elastos.home.browser-state/v1","principal_id":"wrong","localhost_root":"localhost://wrong"}),
        ),
    ] {
        fixture.seed();
        fixture.write(relative, &bad);
        let before = std::fs::read(fixture.location(relative).1).unwrap();
        assert!(!app
            .clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap()
            .status()
            .is_success());
        assert_bounded_eq!(std::fs::read(fixture.location(relative).1).unwrap(), before);
        assert!(!fixture.location(NEW_OBJECT).1.exists());
    }
}

#[tokio::test]
async fn assistant_v2_failed_source_reads_and_invalid_adoption_fail_without_mutation() {
    let fixture = Fixture::new();
    let app = fixture.app();
    fixture.seed();
    let initial = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    let source_path = fixture
        .location(".AppData/ElastOS/HomeAgent/workspace.json")
        .1;
    std::fs::remove_file(&source_path).unwrap();
    std::fs::create_dir(&source_path).unwrap();
    assert!(!app
        .clone()
        .oneshot(request(V2, &fixture.assistant, None))
        .await
        .unwrap()
        .status()
        .is_success());
    assert!(!app
        .clone()
        .oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&initial, json!({"v":1})))
        ))
        .await
        .unwrap()
        .status()
        .is_success());
    assert!(source_path.is_dir());
    assert!(!fixture.location(NEW_OBJECT).1.exists());
    std::fs::remove_dir(source_path).unwrap();
    fixture.seed();
    fixture.write(
        NEW_OBJECT,
        &json!({"schema":"unsupported","revision":1,"document":{}}),
    );
    let before = std::fs::read(fixture.location(NEW_OBJECT).1).unwrap();
    assert!(!app
        .clone()
        .oneshot(request(V2, &fixture.assistant, None))
        .await
        .unwrap()
        .status()
        .is_success());
    let response = app
        .oneshot(request(
            AGENT,
            &fixture.agent,
            Some(json!({"schema":"elastos.home-agent.workspace/v1","if_revision":9,"document":{}})),
        ))
        .await
        .unwrap();
    assert!(!response.status().is_success());
    assert_bounded_eq!(
        std::fs::read(fixture.location(NEW_OBJECT).1).unwrap(),
        before
    );
    assert_bounded_eq!(
        fixture.read(".AppData/ElastOS/HomeAgent/workspace.json")["revision"],
        9
    );
}

#[tokio::test]
async fn assistant_v2_limits_full_retained_envelope_and_preserves_unknown_document_fields() {
    let fixture = Fixture::new();
    let app = fixture.app();
    fixture.seed();
    let initial = json_body(
        app.clone()
            .oneshot(request(V2, &fixture.assistant, None))
            .await
            .unwrap(),
    )
    .await;
    let too_large = json!({"text":"x".repeat(gateway_assistant_workspace_v2::MAX_BYTES - 1000)});
    let response = app
        .clone()
        .oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&initial, too_large)),
        ))
        .await
        .unwrap();
    assert!(!response.status().is_success());
    assert!(!fixture.location(NEW_OBJECT).1.exists());
    for document in [
        json!([]),
        (0..50).fold(json!(0), |value, _| json!({"next":value})),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                V2,
                &fixture.assistant,
                Some(adopt(&initial, document)),
            ))
            .await
            .unwrap();
        assert_bounded_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    // Valid >1MiB demonstrates the route and storage both use the new bound.
    let document = json!({"unrecognizedField":"x".repeat(1024*1024+100),"sessions":(0..50).map(|n|json!({"id":n})).collect::<Vec<_>>()});
    let response = app
        .oneshot(request(
            V2,
            &fixture.assistant,
            Some(adopt(&initial, document.clone())),
        ))
        .await
        .unwrap();
    assert_bounded_eq!(response.status(), StatusCode::OK);
    assert_bounded_eq!(json_body(response).await["document"], document);
}
