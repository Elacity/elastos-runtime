use super::*;
use elastos_runtime::signature::{generate_keypair, SigningKey};

const HOME_CLI_CAPSULE_ID_FOR_TEST: &str = "home-cli";

struct EnvRestore {
    key: &'static str,
    previous: Option<String>,
}

impl EnvRestore {
    fn set(key: &'static str, value: String) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

async fn home_test_get_json(
    app: &axum::Router,
    uri: &str,
    token: &str,
    origin: &'static str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", origin)
                .uri(uri)
                .header("x-elastos-home-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = serde_json::from_slice(&body).unwrap();
    (status, payload)
}

async fn home_test_post_json(
    app: &axum::Router,
    uri: &str,
    token: &str,
    origin: &'static str,
    payload: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", origin)
                .method("POST")
                .uri(uri)
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload = serde_json::from_slice(&body).unwrap();
    (status, payload)
}

pub(super) fn configured_discovery_network_profile_for_test(
    trusted_signing_key: &SigningKey,
    network_id: &str,
) -> crate::collaboration_network::VerifiedCollaborationNetworkProfile {
    let signer_did = crate::crypto::encode_signing_key_did(trusted_signing_key);
    let payload = crate::collaboration_network::CollaborationNetworkProfile {
        schema: crate::collaboration_network::COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
        network_id: network_id.to_string(),
        revision: 1,
        previous_profile_sha256: None,
        signer_did: signer_did.clone(),
        bootstrap_peers: Vec::new(),
        default_conversation: None,
    };
    let payload_bytes =
        crate::collaboration_network::canonical_collaboration_network_profile_payload_bytes(
            &payload,
        )
        .unwrap();
    let (signature, envelope_signer_did) = crate::crypto::domain_separated_sign(
        trusted_signing_key,
        crate::collaboration_network::COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
        &payload_bytes,
    );
    let bytes = serde_json::to_vec(
        &serde_json::to_value(
            crate::collaboration_network::SignedCollaborationNetworkProfile {
                payload,
                signature,
                signer_did: envelope_signer_did,
            },
        )
        .unwrap(),
    )
    .unwrap();
    match crate::collaboration_network::validate_collaboration_network_profile(
        Some(&bytes),
        network_id,
        &[signer_did],
        None,
    )
    .unwrap()
    {
        crate::collaboration_network::CollaborationNetworkProfileMode::Configured(profile) => {
            profile
        }
        crate::collaboration_network::CollaborationNetworkProfileMode::Isolated => {
            panic!("configured discovery profile expected")
        }
    }
}

struct TestCollaborationMessageScope<'a> {
    network_id: &'a str,
    conversation_id: &'a str,
}

fn signed_discovery_message_for_test(
    signing_key: &SigningKey,
    sender_profile_did: &str,
    scope: TestCollaborationMessageScope<'_>,
    recipient: elastos_common::collaboration_protocol::CollaborationRecipient,
    payload_type: &str,
    payload: serde_json::Value,
    validity: std::ops::Range<u64>,
) -> Vec<u8> {
    let message = elastos_common::collaboration_protocol::CollaborationMessage {
        schema: elastos_common::collaboration_protocol::COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
        network_id: scope.network_id.to_string(),
        conversation_id: scope.conversation_id.to_string(),
        message_id: crate::collaboration_core::random_hex_128().unwrap(),
        nonce: crate::collaboration_core::random_hex_128().unwrap(),
        created_at: validity.start,
        expires_at: validity.end,
        sender_profile_did: sender_profile_did.to_string(),
        sender_service: crate::collaboration_discovery::COLLABORATION_DISCOVERY_SERVICE.to_string(),
        recipient,
        payload_type: payload_type.to_string(),
        payload,
    };
    let payload_bytes =
        elastos_common::collaboration_protocol::canonical_collaboration_message_bytes(&message)
            .unwrap();
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        signing_key,
        elastos_common::collaboration_protocol::COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        &payload_bytes,
    );
    elastos_common::collaboration_protocol::canonical_signed_collaboration_message_bytes(
        &elastos_common::collaboration_protocol::SignedCollaborationMessage {
            payload: message,
            signature,
            signer_did,
        },
    )
    .unwrap()
}

fn signed_contact_decision_for_test(
    recipient_key: &SigningKey,
    network_id: &str,
    request_bytes: &[u8],
    recipient_profile_did: &str,
    decided_at: u64,
) -> Vec<u8> {
    let request: elastos_common::collaboration_protocol::SignedCollaborationMessage =
        serde_json::from_slice(request_bytes).unwrap();
    let request_payload: crate::collaboration_discovery::CollaborationContactRequestPayload =
        serde_json::from_value(request.payload.payload.clone()).unwrap();
    let requester_profile = crate::collaboration_profile_authority::verify_signed_profile_document(
        &request_payload.signed_profile,
    )
    .unwrap();
    let payload = crate::collaboration_discovery::CollaborationContactDecisionReceipt {
        schema: crate::collaboration_discovery::COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1
            .to_string(),
        network_id: network_id.to_string(),
        request_envelope_sha256:
            elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(
                request_bytes,
            ),
        conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_ID
            .to_string(),
        requester_profile_did: requester_profile.document().profile_did.clone(),
        requester_endpoint_did: request.signer_did,
        request_message_id: request.payload.message_id,
        request_message_nonce: request.payload.nonce,
        recipient_profile_did: recipient_profile_did.to_string(),
        recipient_endpoint_did: crate::crypto::encode_signing_key_did(recipient_key),
        decision: crate::collaboration_discovery::CollaborationContactDecision::Accepted,
        decided_at,
    };
    let payload_bytes = serde_json::to_vec(&serde_json::to_value(&payload).unwrap()).unwrap();
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        recipient_key,
        crate::collaboration_discovery::COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
        &payload_bytes,
    );
    crate::collaboration_discovery::canonical_signed_collaboration_contact_decision_receipt_bytes(
        &crate::collaboration_discovery::SignedCollaborationContactDecisionReceipt {
            payload,
            signature,
            signer_did,
        },
    )
    .unwrap()
}

fn assert_isolated_launch_route(route: &str, app: &str) -> url::Url {
    let route = url::Url::parse("http://localhost")
        .unwrap()
        .join(route)
        .expect("canonical capsule launch route");
    assert_eq!(route.host_str(), Some("localhost"));
    assert_eq!(route.path(), format!("/apps/{app}/"));
    assert!(
        launch_token_from_route(route.as_str()).is_some(),
        "launch authority must be in the URL fragment: {route}"
    );
    assert!(
        route.query_pairs().all(|(key, _)| key != "home_token"),
        "launch authority must not be in the query: {route}"
    );
    route
}

fn launch_token_from_route(route: &str) -> Option<String> {
    let route = url::Url::parse("http://localhost").ok()?.join(route).ok()?;
    url::form_urlencoded::parse(route.fragment()?.as_bytes())
        .find(|(key, _)| key == "home_token")
        .map(|(_, value)| value.into_owned())
}

fn home_shell_shared_facts(summary: &serde_json::Value) -> serde_json::Value {
    json!({
        "identity": summary["identity"],
        "authority": {
            "principal_id": summary["authority"]["principal_id"],
            "session_id": summary["authority"]["session_id"],
            "proof_binding_id": summary["authority"]["proof_binding_id"],
        },
        "runtime": summary["runtime"],
        "services": summary["services"],
        "capsule_catalog": summary["capsule_catalog"],
        "capsule_interfaces": summary["capsule_interfaces"],
        "targets": summary["targets"],
    })
}

fn file_snapshot(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &std::path::Path, path: &std::path::Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, out);
            } else if entry.file_type().unwrap().is_file() {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

pub(super) fn write_home_principal_object_json_for_authority(
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
    filename: &str,
    value: serde_json::Value,
) {
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let uri = format!("{localhost_root}/.AppData/ElastOS/Home/{filename}");
    let path = elastos_common::localhost::rooted_localhost_fs_path(data_dir, &uri).unwrap();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let relative = parent.strip_prefix(data_dir).unwrap();
            let mut current = data_dir.to_path_buf();
            for component in relative.components() {
                current.push(component.as_os_str());
                std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
    }
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    crate::auth::write_principal_root_object(
        data_dir,
        &authority.principal_id,
        &localhost_root,
        &uri,
        &path,
        &bytes,
    )
    .unwrap();
}

#[tokio::test]
async fn test_home_static_route_serves_browser_surface() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::write(
        dir.path()
            .join("capsules")
            .join(SYSTEM_CAPSULE_ID)
            .join("browser")
            .join("esp-projections.mjs"),
        "export const ok = true;",
    )
    .unwrap();

    let resp = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/apps/home/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .all(|value| !value.starts_with(&format!("{HOME_SESSION_COOKIE}="))),
        "Home index should not auto-mint a local Home session cookie"
    );
    assert_eq!(
        resp.headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Home · ElastOS"));
    assert!(text.contains("./home-shell-host.js"));

    let unsigned_summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unsigned_summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(unsigned_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], false);

    let valid_cookie = format!("{}={}", HOME_SESSION_COOKIE, home_app_token(dir.path()));
    let existing_session = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/home/")
                .header(COOKIE, valid_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(existing_session.status(), StatusCode::OK);
    assert!(
        existing_session
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .all(|value| !value.starts_with(&format!("{HOME_SESSION_COOKIE}="))),
        "valid Home session cookie should not be replaced"
    );

    let asset = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/apps/home/home-shell-host.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        asset
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/javascript")
    );

    let module_asset = app
        .oneshot(
            Request::builder()
                .uri("/apps/system/esp-projections.mjs")
                .header(HOST, "localhost:61180")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(module_asset.status(), StatusCode::OK);
    assert_eq!(
        module_asset
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/javascript")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn test_home_cli_terminal_stream_requires_cli_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let _program = EnvRestore::set("ELASTOS_HOME_CLI_TERMINAL_PROGRAM", "/bin/sh".to_string());
    let _args = EnvRestore::set(
        "ELASTOS_HOME_CLI_TERMINAL_ARGS_JSON",
        serde_json::json!([
            "-c",
            "printf 'ready\\n'; while IFS= read -r line; do printf 'echo:%s\\n' \"$line\"; [ \"$line\" = exit ] && exit 0; done"
        ])
        .to_string(),
    );
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("terminal"));
    let home_token = authority.home_token.clone();
    let cli_token = app_token_for_authority(dir.path(), HOME_CLI_CAPSULE_ID_FOR_TEST, &authority);

    let contract = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home-cli/terminal/contract")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(contract.status(), StatusCode::OK);
    let body = axum::body::to_bytes(contract.into_body(), usize::MAX)
        .await
        .unwrap();
    let contract: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(contract["schema"], "elastos.home-cli.terminal-contract/v1");
    assert_eq!(contract["transport"], "runtime_pty_stream");
    assert!(contract["renderer_contract"]
        .as_str()
        .unwrap()
        .contains("xterm.js"));
    assert!(contract["pty"]
        .as_str()
        .unwrap()
        .contains("Runtime-owned PTY"));
    assert_eq!(contract["input"]["method"], "GET");
    assert!(contract["protocol"].as_str().unwrap().contains("WebSocket"));

    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home-cli/terminal/sessions")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let wrong_app = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home-cli/terminal/sessions")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_app.status(), StatusCode::FORBIDDEN);

    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home-cli/terminal/sessions")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home-cli.terminal-start/v1",
                        "cols": 132,
                        "rows": 36
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let body = axum::body::to_bytes(started.into_body(), usize::MAX)
        .await
        .unwrap();
    let started: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(started["schema"], "elastos.home-cli.terminal-session/v1");
    assert_eq!(started["transport"], "runtime_pty_stream");
    assert_eq!(started["pty"], true);
    assert_eq!(started["authority"]["app"], HOME_CLI_CAPSULE_ID_FOR_TEST);
    assert_eq!(started["dimensions"]["cols"], 132);
    assert_eq!(started["dimensions"]["rows"], 36);
    assert_eq!(started["process"]["mode"], "tui");
    assert_eq!(started["stream"]["schema"], "elastos.runtime.stream/v1");
    assert_eq!(
        started["stream"]["resize_schema"],
        "elastos.home-cli.terminal-resize/v1"
    );
    assert_eq!(
        started["stream"]["intent_schema"],
        "elastos.home-cli.terminal-intent/v1"
    );
    assert!(started["stream"]["resize_url"]
        .as_str()
        .unwrap()
        .ends_with("/resize"));
    let session_id = started["session_id"].as_str().unwrap();
    let events_url = started["stream"]["events_url"].as_str().unwrap();
    let input_socket_url = started["stream"]["input_socket_url"].as_str().unwrap();
    let resize_url = started["stream"]["resize_url"].as_str().unwrap();
    let intent_url = started["stream"]["intent_url"].as_str().unwrap();
    let close_url = started["stream"]["close_url"].as_str().unwrap();
    assert!(events_url.contains(session_id));
    assert!(!events_url.contains("home_token="));
    assert!(events_url.contains("ticket="));
    assert!(input_socket_url.contains(session_id));
    assert!(input_socket_url.contains("ticket=input-"));
    assert!(!input_socket_url.contains("home_token="));
    assert!(intent_url.ends_with("/intent"));
    let input_ticket = input_socket_url.split("ticket=").nth(1).unwrap();
    assert!(home_terminal_input_ticket_matches(
        Some(input_ticket),
        input_ticket
    ));
    assert!(!home_terminal_input_ticket_matches(
        Some("wrong"),
        input_ticket
    ));

    let bad_events = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/apps/home-cli/terminal/sessions/{session_id}/events?ticket=wrong"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_events.status(), StatusCode::FORBIDDEN);

    let non_websocket_input = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(input_socket_url)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(non_websocket_input.status(), StatusCode::BAD_REQUEST);

    let resize = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(resize_url)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home-cli.terminal-resize/v1",
                        "cols": 90,
                        "rows": 24
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resize.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resize.into_body(), usize::MAX)
        .await
        .unwrap();
    let resize: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(resize["schema"], "elastos.home-cli.terminal-resize/v1");
    assert_eq!(resize["dimensions"]["cols"], 90);
    assert_eq!(resize["dimensions"]["rows"], 24);

    let wrong_intent_token = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(intent_url)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home.terminal-host-intent/v1",
                        "action": "open-target",
                        "action_id": "open-gui:browser",
                        "target": "browser"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_intent_token.status(), StatusCode::FORBIDDEN);

    let implicit_intent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(intent_url)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home.terminal-host-intent/v1",
                        "action": "open-target",
                        "target": "browser"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(implicit_intent.status(), StatusCode::BAD_REQUEST);

    let smuggled_query = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(intent_url)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home.terminal-host-intent/v1",
                        "action": "open-target",
                        "action_id": "open-gui:browser",
                        "target": "browser",
                        "query": { "debug": "1" }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(smuggled_query.status(), StatusCode::BAD_REQUEST);

    let authorized_intent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(intent_url)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "schema": "elastos.home.terminal-host-intent/v1",
                        "action": "open-target",
                        "action_id": "open-gui:browser",
                        "target": "browser"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized_intent.status(), StatusCode::OK);
    let body = axum::body::to_bytes(authorized_intent.into_body(), usize::MAX)
        .await
        .unwrap();
    let authorized_intent: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        authorized_intent["schema"],
        "elastos.home-cli.terminal-intent/v1"
    );
    assert_eq!(authorized_intent["session_id"], session_id);
    assert_eq!(
        authorized_intent["intent"]["schema"],
        "elastos.home.terminal-host-intent/v1"
    );
    assert_eq!(authorized_intent["intent"]["action_id"], "open-gui:browser");
    assert!(authorized_intent["intent"]["query"].is_null());

    let closed = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(close_url)
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_home_summary_reports_identity_and_launch_targets() {
    let dir = tempfile::tempdir().unwrap();

    let state = library_test_state(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let library_token = app_token_for_authority(dir.path(), LIBRARY_CAPSULE_ID, &authority);
    let public = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    let public_body = axum::body::to_bytes(public.into_body(), usize::MAX)
        .await
        .unwrap();
    let public_payload: serde_json::Value = serde_json::from_slice(&public_body).unwrap();
    assert_eq!(public_payload["authority"]["signed_in"], false);
    assert_eq!(public_payload["authority"]["principal_id"], "");
    assert_eq!(public_payload["authority"]["session_id"], "");
    assert_eq!(public_payload["authority"]["wallet_connected"], false);
    assert!(public_payload["identity"]["profile"].is_null());
    assert!(public_payload["identity"]["profile_setup_display_name"].is_null());
    assert!(public_payload["identity"]["device_did"].is_null());
    assert_eq!(public_payload["browser_state"]["principal_id"], "");
    assert_eq!(public_payload["browser_state"]["localhost_root"], "");
    assert_eq!(
        public_payload["desktop_objects"]["schema"],
        "elastos.home.desktop-objects/v1"
    );
    assert_eq!(public_payload["desktop_objects"]["uri"], "");
    assert!(public_payload["desktop_objects"]["objects"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(public_payload["browser_state"]["layout"].is_null());
    assert!(public_payload["browser_state"]["session"].is_null());
    assert!(public_payload["browser_state"]["recent_targets"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(public_payload["appearance"]["background_image_url"].is_null());
    assert_eq!(
        public_payload["appearance"]["background_overlay_enabled"],
        false
    );
    assert_eq!(public_payload["runtime"]["running"], false);
    assert_eq!(public_payload["notifications"]["unread_count"], 0);
    assert!(public_payload["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["target"] == "system"));
    assert_eq!(
        public_payload["capsule_catalog"]["schema"],
        "elastos.capsules.catalog/v1"
    );
    assert_eq!(
        public_payload["capsule_interfaces"]["schema"],
        "elastos.capsules.interfaces/v1"
    );
    let catalog_capsules = public_payload["capsule_catalog"]["capsules"]
        .as_array()
        .unwrap();
    for target in public_payload["targets"].as_array().unwrap() {
        let capsule = catalog_capsules
            .iter()
            .find(|capsule| capsule["launch_target"] == target["target"])
            .expect("Home target must come from the canonical capsule catalog");
        assert_eq!(target["title"], capsule["title"]);
        assert_eq!(target["description"], capsule["description"]);
        assert_eq!(target["route"], capsule["route"]);
        assert_eq!(target["role"], capsule["role"]);
        assert_eq!(target["viewer"], capsule["viewer"]);
        assert_eq!(target["viewer_title"], capsule["viewer_title"]);
    }

    let resp = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/provider/object/mkdir")
                .header("x-elastos-home-token", library_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "parent_uri": format!("{}/Desktop", crate::auth::principal_localhost_root(&authority.principal_id)),
                        "name": "Test Folder",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], true);
    assert!(payload["identity"]["profile"].is_null());
    assert_eq!(payload["identity"]["profile_setup_display_name"], "anders");
    // Decided under invariant 1: the local device DID reaches exactly one
    // browser surface, System (asserted below at /api/apps/system/summary).
    // The Home shell has no consumer for it, so the Home summary strips it.
    assert!(payload["identity"]["device_did"].is_null());
    assert_eq!(payload["home"]["route"], "/apps/home/");
    assert_eq!(payload["home"]["attach_kind"], "iframe");
    assert_eq!(payload["app"]["id"], "home");
    assert_eq!(payload["app"]["route"], "/apps/home/");
    assert!(payload["appearance"]["background_image_url"].is_null());
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["site"]["root_uri"], MY_WEBSITE_URI);
    assert_eq!(payload["room"]["pending_count"], 0);
    assert_eq!(payload["notifications"]["unread_count"], 0);
    assert_eq!(
        payload["desktop_objects"]["schema"],
        "elastos.home.desktop-objects/v1"
    );
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    assert_eq!(
        payload["desktop_objects"]["uri"],
        format!("{localhost_root}/Desktop")
    );
    assert!(payload["desktop_objects"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| object["name"] == "Test Folder" && object["kind"] == "directory"));
    assert!(payload["desktop_objects"]["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|object| {
            object["name"] == "Trash"
                && object["kind"] == "directory"
                && object["uri"] == format!("{localhost_root}/.Trash")
                && object["metadata"]["system_kind"] == "trash"
        }));
    let targets = payload["targets"].as_array().unwrap();
    let system = targets
        .iter()
        .find(|target| target["target"] == "system")
        .expect("system target");
    assert_eq!(system["role"], "app");
    assert_eq!(system["title"], "System");
    assert_eq!(
        system["description"],
        "Manage passkeys, appearance, and Home settings."
    );
    assert_eq!(system["route"], "/apps/system/");
    assert_eq!(system["attach_kind"], "iframe");
    assert_eq!(system["target_kind"], "app");
    let services = targets
        .iter()
        .find(|target| target["target"] == "services")
        .expect("services target");
    assert_eq!(services["role"], "app");
    assert_eq!(services["title"], "Services");
    assert_eq!(
        services["description"],
        "Manage Browser Exit Node sharing and subscriptions."
    );
    assert_eq!(services["route"], "/apps/services/");
    assert_eq!(services["attach_kind"], "iframe");
    assert_eq!(services["target_kind"], "app");
    assert!(targets
        .iter()
        .any(|target| target["target"] == "chat-room" && target["role"] == "app"));
    let library = targets
        .iter()
        .find(|target| target["target"] == "library")
        .expect("library target");
    assert_eq!(library["role"], "app");
    assert_eq!(library["title"], "Library");
    assert_eq!(
        library["description"],
        "Browse documents and open them in Documents."
    );
    assert_eq!(library["route"], "/apps/library/");
    assert_eq!(library["attach_kind"], "iframe");
    assert_eq!(library["target_kind"], "app");
    let inbox = targets
        .iter()
        .find(|target| target["target"] == "inbox")
        .expect("inbox target");
    assert_eq!(inbox["role"], "app");
    assert_eq!(inbox["title"], "Inbox");
    assert_eq!(
        inbox["description"],
        "Review messages, requests, and approvals."
    );
    assert_eq!(inbox["route"], "/apps/inbox/");
    assert_eq!(inbox["attach_kind"], "iframe");
    assert_eq!(inbox["target_kind"], "app");
    let wallet = targets
        .iter()
        .find(|target| target["target"] == "wallet")
        .expect("wallet target");
    assert_eq!(wallet["role"], "app");
    assert_eq!(wallet["title"], "Wallet");
    assert_eq!(
        wallet["description"],
        "View accounts, balances, approvals, and approval methods."
    );
    assert_eq!(wallet["route"], "/apps/wallet/");
    assert_eq!(wallet["attach_kind"], "iframe");
    assert_eq!(wallet["target_kind"], "app");
    let browser = targets
        .iter()
        .find(|target| target["target"] == "browser")
        .expect("browser target");
    assert_eq!(browser["role"], "app");
    assert_eq!(browser["title"], "Browser");
    assert_eq!(browser["description"], "Browse websites from this device.");
    assert_eq!(browser["route"], "/apps/browser/");
    assert_eq!(browser["attach_kind"], "iframe");
    assert_eq!(browser["target_kind"], "app");
    assert!(targets
        .iter()
        .all(|target| target["target"] != "wallet-metamask"));
    assert!(targets
        .iter()
        .all(|target| target["target"] != "wallet-unisat"));
    assert!(targets
        .iter()
        .all(|target| target["target"] != "wallet-walletconnect"));
    assert!(targets
        .iter()
        .all(|target| target["target"] != "marketplace"));
    let active_capsules = crate::api::capsule_inventory::list_active_capsule_manifests(dir.path())
        .into_iter()
        .map(|manifest| manifest.name)
        .collect::<BTreeSet<_>>();
    assert!(targets.iter().all(|target| {
        target["target"]
            .as_str()
            .is_some_and(|name| active_capsules.contains(name))
    }));
    assert!(targets
        .iter()
        .any(|target| target["target"] == "gba-ucity" && target["target_kind"] == "object"));
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "system")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "library")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "chat-room")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "inbox")
            .count(),
        1
    );
    assert_eq!(
        targets
            .iter()
            .filter(|target| target["target"] == "services")
            .count(),
        1
    );
}

#[tokio::test]
async fn test_services_summary_requires_services_token_and_reports_browser_exit() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/exit-provider.json"), "{}").unwrap();

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let ok = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header(
                    "x-elastos-home-token",
                    issue_home_launch_token(dir.path(), SERVICES_CAPSULE_ID).unwrap(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.runtime.services/v1");
    assert_eq!(payload["local_offer_count"], 0);
    assert_eq!(payload["local_offers"].as_array().unwrap().len(), 0);
    assert!(payload["available_local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| {
            offer["offer_id"] == "local:provider:browser-exit"
                && offer["display_name"] == "Browser Exit node"
                && offer["service_kind"] == "remote_exit"
                && offer["enabled"] == false
                && offer["status"] == "available"
        }));

    let enabled = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header(
                    "x-elastos-home-token",
                    issue_home_launch_token(dir.path(), SERVICES_CAPSULE_ID).unwrap(),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let body = axum::body::to_bytes(enabled.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["local_offer_count"], 1);
    assert!(payload["local_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| {
            offer["offer_id"] == "local:provider:browser-exit" && offer["enabled"] == true
        }));
}

#[tokio::test]
async fn test_services_summary_projects_configured_remote_exit_without_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/exit-provider.json"),
        serde_json::to_vec_pretty(&json!({
            "backends": [],
            "remote_carrier_exits": [{
                "id": "mac-browser-exit",
                "grant_id": "operator-grant:mac-browser-exit:test",
                "peer_did": "did:key:z6Mkmac",
                "carrier_service": "elastos://exit/open_stream",
                "connect_ticket": "ticket:must-not-leak",
                "allowed_principals": ["person:local:test"],
                "allowed_hosts": ["*"],
                "allowed_schemes": ["tcp", "tls"],
                "allowed_ports": [80, 443],
                "max_active_streams": 4,
                "max_active_streams_per_principal": 2
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let token = issue_home_launch_token(dir.path(), SERVICES_CAPSULE_ID).unwrap();

    let ok = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("connect_ticket"));
    assert!(!text.contains("ticket:must-not-leak"));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["remote_offer_count"], 1);
    assert_eq!(payload["available_remote_offer_count"], 0);
    let offer = payload["remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["source"] == "configured_remote_exit")
        .expect("configured remote Exit should be projected as active");
    assert_eq!(offer["offer_id"], "configured:remote-exit:mac-browser-exit");
    assert_eq!(offer["service_kind"], "remote_exit");
    assert_eq!(offer["status"], "active");
    assert_eq!(offer["enabled"], true);
    assert_eq!(offer["grant_required"], false);

    let snapshot = home_services_snapshot(dir.path());
    let snapshot_text = snapshot.to_string();
    assert!(!snapshot_text.contains("connect_ticket"));
    assert!(!snapshot_text.contains("ticket:must-not-leak"));
    assert_eq!(snapshot["remote_offer_count"], 1);
    assert!(snapshot["remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["offer_id"] == "configured:remote-exit:mac-browser-exit"));

    let browser_token = issue_home_launch_token(dir.path(), BROWSER_CAPSULE_ID).unwrap();
    let browser = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/browser/summary")
                .header("x-elastos-home-token", browser_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(browser.status(), StatusCode::OK);
    let body = axum::body::to_bytes(browser.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("peer_did"));
    assert!(!text.contains("connect_ticket"));

    let remove = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"configured:remote-exit:mac-browser-exit","section":"others","selected":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = remove.status();
    let body = axum::body::to_bytes(remove.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("managed by Exit Provider config"));
}

#[tokio::test]
async fn test_services_selection_state_is_principal_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/exit-provider.json"), "{}").unwrap();
    let admin = passkey_authority_with_name(dir.path(), Some("alice"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("bob"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_token = app_token_for_authority(dir.path(), SERVICES_CAPSULE_ID, &admin);
    let guest_token = app_token_for_authority(dir.path(), SERVICES_CAPSULE_ID, &guest);

    let admin_enabled = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", admin_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_enabled.status(), StatusCode::OK);

    let guest_default = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", guest_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_default.status(), StatusCode::OK);
    let body = axum::body::to_bytes(guest_default.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["local_offer_count"], 0);

    let guest_saved_empty = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", guest_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_saved_empty.status(), StatusCode::OK);

    let admin_still_enabled = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", admin_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_still_enabled.status(), StatusCode::OK);
    let body = axum::body::to_bytes(admin_still_enabled.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["local_offer_count"], 1);
    assert!(!dir.path().join("config/services-state.json").exists());
}

#[tokio::test]
async fn test_home_summary_ignores_services_state_left_unencrypted_before_root_protection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/exit-provider.json"), "{}").unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("alice"));
    let app = gateway_router(test_state(dir.path()));
    let services_token = app_token_for_authority(dir.path(), SERVICES_CAPSULE_ID, &authority);

    let selected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", services_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);

    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["services"]["local_offer_count"], 0);
}

#[tokio::test]
async fn test_home_summary_ignores_invalid_protected_services_state() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/exit-provider.json"), "{}").unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("alice"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);

    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let object_uri = format!("{localhost_root}/.AppData/ElastOS/Home/services-state.json");
    let path = rooted_localhost_fs_path(dir.path(), &object_uri).unwrap();
    crate::auth::write_principal_root_object(
        dir.path(),
        &authority.principal_id,
        &localhost_root,
        &object_uri,
        &path,
        b"{not-json",
    )
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["services"]["local_offer_count"], 0);
}

#[tokio::test]
async fn test_services_remote_exit_request_delivers_provider_inbox_notification() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "services-left").await;
    let _right_runtime = start_fake_runtime(right.path(), bus, "services-right").await;
    let left_app = gateway_router(test_state(left.path()));
    let right_app = gateway_router(test_state(right.path()));
    let left_authority = passkey_authority_with_name(left.path(), Some("Alice"));
    let right_authority = passkey_authority_with_name(right.path(), Some("Bob"));
    crate::auth::store_test_principal_root_protection(left.path(), &left_authority.principal_id);
    crate::auth::store_test_principal_root_protection(right.path(), &right_authority.principal_id);
    let (_, left_did) = elastos_identity::load_or_create_did(left.path()).unwrap();
    let (_, right_did) = elastos_identity::load_or_create_did(right.path()).unwrap();

    for (app, token, body) in [
        (
            left_app.clone(),
            left_authority.people_token.as_str(),
            r#"{"display_name":"Alice"}"#,
        ),
        (
            right_app.clone(),
            right_authority.people_token.as_str(),
            r#"{"display_name":"Bob"}"#,
        ),
    ] {
        let response = app
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/people/profile")
                    .header("x-elastos-home-token", token)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    std::fs::create_dir_all(right.path().join("config")).unwrap();
    std::fs::write(right.path().join("config/exit-provider.json"), "{}").unwrap();

    write_home_principal_object_json_for_authority(
        left.path(),
        &left_authority,
        "services-peer-contacts.json",
        json!({
            "schema": "elastos.services.peer-contacts-state/v1",
            "principal_id": left_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&left_authority.principal_id),
            "updated_at": 10,
            "contacts": {
                "contact:right": {
                    "contact_id": "contact:right",
                    "peer_id": "services-right",
                    "did": right_did,
                    "display_name": "Bob",
                    "handle": "Bob",
                    "added_at": 10,
                    "updated_at": 10,
                    "source": "people_discovery"
                }
            }
        }),
    );
    write_home_principal_object_json_for_authority(
        right.path(),
        &right_authority,
        "services-peer-contacts.json",
        json!({
            "schema": "elastos.services.peer-contacts-state/v1",
            "principal_id": right_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&right_authority.principal_id),
            "updated_at": 10,
            "contacts": {
                "contact:left": {
                    "contact_id": "contact:left",
                    "peer_id": "services-left",
                    "did": left_did,
                    "display_name": "Alice",
                    "handle": "Alice",
                    "added_at": 10,
                    "updated_at": 10,
                    "source": "people_discovery"
                }
            }
        }),
    );

    let right_services_token =
        app_token_for_authority(right.path(), SERVICES_CAPSULE_ID, &right_authority);
    let right_shared = right_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", right_services_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"offer_id":"local:provider:browser-exit","section":"mine","selected":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(right_shared.status(), StatusCode::OK);

    let left_services_token =
        app_token_for_authority(left.path(), SERVICES_CAPSULE_ID, &left_authority);
    let left_services = left_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_services.status(), StatusCode::OK);
    let body = axum::body::to_bytes(left_services.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let offer_id = payload["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["service_uri"] == "elastos://peer/browser-exit")
        .and_then(|offer| offer["offer_id"].as_str())
        .unwrap()
        .to_string();

    let requested = left_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", left_services_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "offer_id": offer_id,
                        "section": "others",
                        "selected": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = requested.status();
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));

    let right_inbox_token =
        app_token_for_authority(right.path(), INBOX_CAPSULE_ID, &right_authority);
    let (launch_status, _) = home_test_post_json(
        &right_app,
        "/api/apps/home/launch",
        &right_authority.home_token,
        "http://localhost:61180",
        json!({ "target": INBOX_CAPSULE_ID }),
    )
    .await;
    assert_eq!(launch_status, StatusCode::OK);
    let inbox = right_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", right_inbox_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(inbox.into_body(), usize::MAX)
        .await
        .unwrap();
    let inbox_text = String::from_utf8_lossy(&body);
    assert!(!inbox_text.contains("connect_ticket"));
    assert!(!inbox_text.contains("ticket:"));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let notification = payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "service_access_request")
        .expect("provider inbox should contain a service access request");
    assert!(notification["title"]
        .as_str()
        .unwrap_or_default()
        .contains("Alice"));
    let action_id = notification["action_ref"]["action_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(action_id.starts_with("service-approve-request:"));

    let approved = right_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/inbox/actions")
                .header("x-elastos-home-token", right_inbox_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "action_id": action_id }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = approved.status();
    let body = axum::body::to_bytes(approved.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["message"]
        .as_str()
        .unwrap_or_default()
        .contains("private remote Exit grant was sent"));

    let inbox_after = right_app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/inbox/summary")
                .header("x-elastos-home-token", right_inbox_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(inbox_after.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!payload["notifications"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["kind"] == "service_access_request"));

    let left_summary_after = left_app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_summary_after.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("connect_ticket"));
    assert!(!text.contains("ticket:"));
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let approved_offer = payload["remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["offer_id"] == offer_id)
        .expect("requester should keep the requested service selected");
    assert_eq!(approved_offer["status"], "active");
    assert_eq!(approved_offer["enabled"], true);
    assert_eq!(approved_offer["grant_required"], false);
    assert_eq!(
        approved_offer["grant_scope"],
        "installed_remote_carrier_exit_grant"
    );
    assert_eq!(approved_offer["route"], "/apps/browser/");
    let exit_config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(left.path().join("config/exit-provider.json")).unwrap(),
    )
    .unwrap();
    let installed_exit = exit_config["remote_carrier_exits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|exit| {
            exit["connect_ticket"] == "fake-ticket-services-right"
                && exit["peer_did"] == "services-right"
        })
        .expect("approval should install a private remote Carrier Exit grant");
    assert_eq!(
        installed_exit["allowed_principals"][0],
        left_authority.principal_id
    );
    assert_eq!(installed_exit["allowed_ports"], json!([80, 443]));
}

#[tokio::test]
async fn test_services_remote_exit_request_local_only_does_not_save_requested_state() {
    let left = tempfile::tempdir().unwrap();
    let right = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _left_runtime = start_fake_runtime(left.path(), bus.clone(), "services-left-local").await;
    let _right_runtime =
        start_fake_runtime(right.path(), bus.clone(), "services-right-local").await;
    let left_app = gateway_router(test_state(left.path()));
    let left_authority = passkey_authority_with_name(left.path(), Some("Alice"));
    let right_authority = passkey_authority_with_name(right.path(), Some("Bob"));
    crate::auth::store_test_principal_root_protection(left.path(), &left_authority.principal_id);
    crate::auth::store_test_principal_root_protection(right.path(), &right_authority.principal_id);
    let right_did = elastos_identity::load_or_create_did(right.path())
        .unwrap()
        .1;

    write_home_principal_object_json_for_authority(
        left.path(),
        &left_authority,
        "services-peer-contacts.json",
        json!({
            "schema": "elastos.services.peer-contacts-state/v1",
            "principal_id": left_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&left_authority.principal_id),
            "updated_at": 10,
            "contacts": {
                "contact:right": {
                    "contact_id": "contact:right",
                    "peer_id": "services-right-local",
                    "did": right_did,
                    "display_name": "Bob",
                    "handle": "Bob",
                    "added_at": 10,
                    "updated_at": 10,
                    "source": "people_discovery"
                }
            }
        }),
    );
    write_home_principal_object_json_for_authority(
        right.path(),
        &right_authority,
        "services-peer-contacts.json",
        json!({
            "schema": "elastos.services.peer-contacts-state/v1",
            "principal_id": right_authority.principal_id,
            "localhost_root": crate::auth::principal_localhost_root(&right_authority.principal_id),
            "updated_at": 10,
            "contacts": {}
        }),
    );

    let profile = left_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/people/profile")
                .header("x-elastos-home-token", left_authority.people_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"Alice"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(profile.status(), StatusCode::OK);

    let left_services_token =
        app_token_for_authority(left.path(), SERVICES_CAPSULE_ID, &left_authority);
    let left_services = left_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(left_services.status(), StatusCode::OK);
    let body = axum::body::to_bytes(left_services.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let offer_id = payload["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|offer| offer["service_uri"] == "elastos://peer/browser-exit")
        .and_then(|offer| offer["offer_id"].as_str())
        .unwrap()
        .to_string();

    bus.lock()
        .await
        .local_only_message_substrings
        .push("service_access_request".to_string());
    let requested = left_app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/services/offers")
                .header("x-elastos-home-token", left_services_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "offer_id": offer_id,
                        "section": "others",
                        "selected": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = requested.status();
    let body = axum::body::to_bytes(requested.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(String::from_utf8_lossy(&body).contains("not delivered"));

    let left_services_after = left_app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/services/summary")
                .header("x-elastos-home-token", left_services_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(left_services_after.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["remote_offers"].as_array().unwrap().is_empty());
    assert!(payload["available_remote_offers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|offer| offer["offer_id"] == offer_id));
}

#[tokio::test]
async fn test_home_summary_does_not_turn_conversation_members_into_people_contacts() {
    let dir = tempfile::tempdir().unwrap();
    let guest = tempfile::tempdir().unwrap();
    let state = library_test_state(dir.path()).await;
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/browser-engine-adapter.json"),
        serde_json::to_vec(&serde_json::json!({
            "adapters": [{
                "id": "browser-vm-product",
                "kind": "chromium_microvm",
                "network_mode": "runtime_net_only",
                "display_modes": ["webrtc_remote_display"],
                "supervisor": {
                    "program": "/tmp/browser-vm-engine-supervisor",
                    "control_socket_path": "/tmp/elastos-browser-vm-control-test.sock"
                }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/ipfs-provider"), "").unwrap();
    let (_owner_device_did, owner_profile) =
        test_signed_room_profile(dir.path(), 31, "Owner", Some("owner"));
    let (_guest_device_did, guest_profile) =
        test_signed_room_profile(guest.path(), 32, "Guest", Some("guest"));
    let guest_profile_did = guest_profile.document().profile_did.clone();

    crate::room_service::seed_room_owner(
        dir.path(),
        &owner_profile,
        crate::room_service::RoomOwnerSeedInput {
            title: "Chat".to_string(),
        },
    )
    .unwrap();
    let invite = crate::room_service::export_room_invite_envelope(
        dir.path(),
        crate::room_service::RoomInviteInput {
            invited_profile_did: guest_profile_did.clone(),
            role: crate::room_service::RoomRole::Member,
        },
        &owner_profile,
    )
    .unwrap();
    let imported = crate::room_service::import_room_invite_envelope(
        guest.path(),
        &serde_json::to_vec(&invite).unwrap(),
        &guest_profile,
    )
    .unwrap();
    crate::room_service::accept_room_invite_for_test(
        guest.path(),
        &guest_profile_did,
        &imported.invite_id,
    )
    .unwrap();
    let acceptance = crate::room_service::export_room_acceptance_envelope(
        guest.path(),
        &imported.invite_id,
        &guest_profile,
    )
    .unwrap();
    crate::room_service::import_room_acceptance_envelope(
        dir.path(),
        &serde_json::to_vec(&acceptance).unwrap(),
    )
    .unwrap();

    let resp = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["people"]["schema"], "elastos.people.contacts/v1");
    assert_eq!(payload["people"]["contact_count"], 0);
    assert!(payload["people"]["contacts"].as_array().unwrap().is_empty());
    assert!(payload["people"]["service_offers"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_people_profile_creation_requires_completed_system_recovery_without_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let _ = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let app = gateway_router(wallet_test_state(dir.path()).await);
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    assert!(
        crate::auth::load_principal_root_protection(
            dir.path(),
            &principal.principal_id,
            &principal.localhost_root,
        )
        .unwrap()
        .is_none(),
        "this journey starts with no protection"
    );
    let before_profile_attempt = file_snapshot(dir.path());

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .method("POST")
                    .uri("/api/apps/people/profile")
                    .header("x-elastos-home-token", authority.people_token.as_str())
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"display_name":"Anders"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            payload,
            json!({
                "schema": "elastos.people.profile-protection-required/v1",
                "status": "recovery_required",
                "action_target": "system",
                "message": "Open System, choose Security, and download Recovery. Then retry creating your Profile."
            })
        );
        assert_eq!(file_snapshot(dir.path()), before_profile_attempt);
    }
    assert!(
        crate::collaboration_profile_authority::load_profile_authority(
            dir.path(),
            &principal.principal_id,
            &principal.localhost_root,
        )
        .unwrap()
        .is_none()
    );
    assert!(crate::auth::load_principal_root_protection(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .is_none());

    let export_intent = json!({
        "principal_id": authority.principal_id,
        "localhost_root": principal.localhost_root,
        "label": "Recovery Kit",
        "download_password": null,
    });
    let step_up = step_up_token_for_app_context(
        dir.path(),
        SYSTEM_CAPSULE_ID,
        &authority.system_token,
        "auth.full-recovery-bundle.export",
        &export_intent,
    );
    let export = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/auth/recovery/full-export")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "schema": "elastos.full-recovery-bundle.export.request/v1",
                        "principal_id": export_intent["principal_id"],
                        "localhost_root": export_intent["localhost_root"],
                        "label": export_intent["label"],
                        "step_up_token": step_up,
                        "download_password": null,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    let protection = crate::auth::load_principal_root_protection(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
    )
    .unwrap()
    .expect("System Recovery establishes root protection");
    assert!(protection
        .protectors
        .iter()
        .any(|protector| protector.verified_at.is_some()));

    let profile = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/people/profile")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"Anders"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(profile.status(), StatusCode::OK);

    let restarted = gateway_router(test_state(dir.path()));
    let summary = restarted
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/people/summary")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["identity"]["profile_readiness"]["status"], "ready");
    assert_eq!(payload["identity"]["profile"]["display_name"], "Anders");
}

#[tokio::test]
async fn test_people_invite_create_route_is_absent_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let profile = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/people/profile")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"Anders"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(profile.status(), StatusCode::OK);
    let before = file_snapshot(dir.path());

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/people/invites/create")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(file_snapshot(dir.path()), before);
}

#[tokio::test]
async fn test_people_contact_remove_requires_profile_contact_authority() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));

    let response = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/people/contacts/remove")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "contact_id": "contact:missing" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("Save your Profile before using People contacts"));
}

#[tokio::test]
async fn test_people_contact_remove_removes_exact_profile_contact_locally() {
    const NETWORK: &str = "gateway-profile-contact-remove";
    let now = crate::auth::now_ts();
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "profile-contact-remove").await;
    let authority = passkey_authority_with_name(dir.path(), Some("Local"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);

    let (trusted_key, _) = generate_keypair();
    let network_profile = configured_discovery_network_profile_for_test(&trusted_key, NETWORK);
    // Production keys the discovery service with the runtime device identity,
    // which is the device the Profile authorizes — a revocation must be
    // signed by an authorized device, so the test wires the same key.
    let (runtime_device_key, _) = elastos_identity::load_or_create_did(dir.path()).unwrap();
    let discovery_service =
        crate::collaboration_discovery_runtime::CollaborationDiscoveryService::new(
            SigningKey::from_bytes(&runtime_device_key.to_bytes()),
            network_profile.clone(),
            Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
        )
        .await
        .unwrap();
    let mut state = test_state(dir.path());
    state.collaboration_discovery_service = Some(discovery_service);
    let app = gateway_router(state);

    let (status, _) = home_test_post_json(
        &app,
        "/api/apps/people/profile",
        &authority.people_token,
        "null",
        serde_json::json!({ "display_name": "Local Profile" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let local_profile = crate::collaboration_profile_authority::load_profile_authority(
        dir.path(),
        &authority.principal_id,
        &localhost_root,
    )
    .unwrap()
    .unwrap();
    let local_device_seed: [u8; 32] = std::fs::read(dir.path().join("identity/device.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let (local_device_key, _) = elastos_identity::derive_did(&local_device_seed);
    let local_device_did =
        crate::collaboration_profile_authority::load_existing_device_did(dir.path())
            .unwrap()
            .unwrap();
    let store = crate::collaboration_contact_store::CollaborationContactStore::new(
        dir.path(),
        &authority.principal_id,
        &localhost_root,
        network_profile,
        &local_profile,
        &local_device_did,
    )
    .unwrap();

    let (remote_device_key, _) = generate_keypair();
    let remote_device_key = SigningKey::from_bytes(&remote_device_key.to_bytes());
    let remote_device_did = crate::crypto::encode_signing_key_did(&remote_device_key);
    let (remote_profile_key, _) = generate_keypair();
    let remote_profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
        &SigningKey::from_bytes(&remote_profile_key.to_bytes()),
        "Remote Profile",
        Some("remote"),
        1,
        None,
        now,
        vec![remote_device_did.clone()],
    )
    .unwrap();
    let remote_advertisement = signed_discovery_message_for_test(
        &remote_device_key,
        &remote_profile.document().profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Conversation,
            id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationDiscoveryAdvertisementPayload {
                signed_profile: remote_profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        now..now + crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
    );
    let request = signed_discovery_message_for_test(
        &local_device_key,
        &local_profile.document().profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Profile,
            id: remote_profile.document().profile_did.clone(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationContactRequestPayload {
                advertisement_envelope_sha256:
                    elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(
                        &remote_advertisement,
                    ),
                signed_profile: local_profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        (now + 1)
            ..(now
                + 1
                + crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS),
    );
    store
        .record_outgoing_contact_request(&request, &remote_advertisement, now + 1)
        .unwrap();
    store
        .record_contact_decision_receipt(
            &signed_contact_decision_for_test(
                &remote_device_key,
                NETWORK,
                &request,
                &remote_profile.document().profile_did,
                now + 2,
            ),
            now + 2,
        )
        .unwrap();

    let (status, people) = home_test_get_json(
        &app,
        "/api/apps/people/summary",
        &authority.people_token,
        "null",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(people["people"]["contact_count"], 1);
    let conversation_id = people["people"]["contacts"][0]["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let contact_id = people["people"]["contacts"][0]["contact_id"]
        .as_str()
        .unwrap()
        .to_string();
    let before = file_snapshot(dir.path());

    let (status, response) = home_test_post_json(
        &app,
        "/api/apps/people/contacts/remove",
        &authority.people_token,
        "null",
        serde_json::json!({ "contact_id": contact_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["scope"], "profile_contact");
    let after = file_snapshot(dir.path());
    for (path, bytes) in before {
        if !path.ends_with("/.AppData/ElastOS/People/contact-state.json") {
            assert_eq!(
                after.get(&path),
                Some(&bytes),
                "unexpected mutation: {path}"
            );
        }
    }

    let (status, people) = home_test_get_json(
        &app,
        "/api/apps/people/summary",
        &authority.people_token,
        "null",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Removal is visible, not a disappearance: the pair stays as a removed
    // relationship with messaging off, named by its signed presentation.
    assert_eq!(people["people"]["contact_count"], 1);
    assert_eq!(people["people"]["contacts"][0]["relationship"], "removed");
    assert_eq!(people["people"]["contacts"][0]["can_message"], false);
    assert!(people["people"]["contacts"][0]["conversation_id"].is_string());

    let local_readd_advertisement = signed_discovery_message_for_test(
        &local_device_key,
        &local_profile.document().profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Conversation,
            id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationDiscoveryAdvertisementPayload {
                signed_profile: local_profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        (now + 3)
            ..(now
                + 3
                + crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS),
    );
    store
        .store_local_advertisement(&local_readd_advertisement, now + 3)
        .unwrap();

    let readd_request = signed_discovery_message_for_test(
        &remote_device_key,
        &remote_profile.document().profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Profile,
            id: local_profile.document().profile_did.clone(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationContactRequestPayload {
                advertisement_envelope_sha256:
                    elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(
                        &local_readd_advertisement,
                    ),
                signed_profile: remote_profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        (now + 4)
            ..(now
                + 4
                + crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS),
    );
    store
        .record_incoming_contact_request(&readd_request, now + 4)
        .unwrap();
    store
        .record_contact_decision_receipt(
            &signed_contact_decision_for_test(
                &local_device_key,
                NETWORK,
                &readd_request,
                &local_profile.document().profile_did,
                now + 5,
            ),
            now + 5,
        )
        .unwrap();

    let (status, people) = home_test_get_json(
        &app,
        "/api/apps/people/summary",
        &authority.people_token,
        "null",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(people["people"]["contact_count"], 1);
    assert_eq!(people["people"]["contacts"][0]["relationship"], "connected");
    assert_eq!(people["people"]["contacts"][0]["can_message"], true);
    assert_eq!(
        people["people"]["contacts"][0]["conversation_id"],
        conversation_id
    );
    assert_eq!(
        people["people"]["contacts"][0]["display_name"],
        "Remote Profile"
    );
}

#[tokio::test]
async fn test_home_events_long_poll_returns_cursor_and_keepalive() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("events"));

    let first = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/events?wait_ms=0")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let first_json: serde_json::Value = serde_json::from_slice(&first_body).unwrap();
    assert_eq!(first_json["schema"], "elastos.home.events/v1");
    assert_eq!(first_json["keepalive"], false);
    assert!(first_json["cursor"].as_str().unwrap().starts_with("v1:"));
    assert!(first_json["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "home.summary.changed"));

    let cursor = first_json["cursor"].as_str().unwrap();
    let second = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri(format!("/api/apps/home/events?wait_ms=0&cursor={cursor}"))
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let second_json: serde_json::Value = serde_json::from_slice(&second_body).unwrap();
    assert_eq!(second_json["schema"], "elastos.home.events/v1");
    assert_eq!(second_json["cursor"], cursor);
    assert_eq!(second_json["keepalive"], true);
    assert!(second_json["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_home_events_stream_requires_home_authority_and_serves_sse() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("events"));

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/events/stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);

    let authorized = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/events/stream")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
    assert!(
        authorized
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "Home event stream should be served as SSE"
    );
    assert_eq!(
        authorized
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache, no-transform"),
        "Home SSE must not be cached or transformed by proxies"
    );
    assert_eq!(
        authorized
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "nginx must not buffer realtime Home events"
    );
}

#[tokio::test]
async fn test_home_summary_and_events_include_browser_wallet_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority(dir.path());
    let browser_token = app_token_for_authority(dir.path(), BROWSER_CAPSULE_ID, &authority);
    let address = "0x1111111111111111111111111111111111111111";
    let account_id = format!("wallet:eip155:20:{address}");
    let provider = MockWalletProvider {
        challenges: TokioMutex::default(),
        bitcoin_challenges: TokioMutex::default(),
        accounts: TokioMutex::new(vec![json!({
            "account_id": account_id,
            "principal_id": authority.principal_id,
            "proof_binding_id": "proof:wallet:managed:eip155:20:0x1111111111111111111111111111111111111111",
            "chain_namespace": "eip155:20",
            "address": address,
            "proof_type": "managed_evm",
            "label": "Spending",
            "linked_at": crate::auth::now_ts()
        })]),
        approvals: TokioMutex::default(),
        defaults: TokioMutex::default(),
    };
    let app =
        gateway_router(wallet_chain_test_state_with_wallet_provider(dir.path(), provider).await);

    let before = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/events?wait_ms=0")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);
    let before_body = axum::body::to_bytes(before.into_body(), usize::MAX)
        .await
        .unwrap();
    let before_json: serde_json::Value = serde_json::from_slice(&before_body).unwrap();
    let cursor = before_json["cursor"].as_str().unwrap().to_string();

    let request = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/browser/wallet/request-transaction")
                .header("x-elastos-home-token", browser_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{
                        "method":"eth_sendTransaction",
                        "params":[{{"from":"{address}","to":"0x2222222222222222222222222222222222222222","value":"0x1","data":"0x"}}],
                        "account_id":"wallet:eip155:20:{address}",
                        "chain_namespace":"eip155:20",
                        "address":"{address}",
                        "page_url":"https://ela.city/",
                        "origin":"https://ela.city"
                    }}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(request.status(), StatusCode::OK);

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_json: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(summary_json["notifications"]["attention_count"], 1);
    assert_eq!(
        summary_json["notifications"]["entries"][0]["title"],
        "Transaction approval request"
    );

    let events = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri(format!("/api/apps/home/events?wait_ms=0&cursor={cursor}"))
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    let events_body = axum::body::to_bytes(events.into_body(), usize::MAX)
        .await
        .unwrap();
    let events_json: serde_json::Value = serde_json::from_slice(&events_body).unwrap();
    assert!(events_json["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| { event["kind"] == "wallet.requests.changed" && event["scope"] == "wallet" }));
}

#[tokio::test]
async fn test_system_updates_home_background_image() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let admin = passkey_authority_with_name(dir.path(), Some("admin"));
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &admin.principal_id);
    let guest_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &guest.principal_id);

    let updated = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", admin.system_token.clone())
                .header(CONTENT_TYPE, "image/png")
                .body(Body::from("admin-image"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = axum::body::to_bytes(updated.into_body(), usize::MAX)
        .await
        .unwrap();
    let updated_payload: serde_json::Value = serde_json::from_slice(&updated_body).unwrap();
    let background_url = updated_payload["background_image_url"]
        .as_str()
        .expect("background url");
    assert!(
        background_url.starts_with("/api/apps/home/appearance/background-image?scope="),
        "{background_url}"
    );
    assert!(background_url.contains("&v="), "{background_url}");

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", admin.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let summary_body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let summary_payload: serde_json::Value = serde_json::from_slice(&summary_body).unwrap();
    assert_eq!(
        summary_payload["appearance"]["background_image_url"],
        updated_payload["background_image_url"]
    );
    assert_eq!(
        summary_payload["appearance"]["background_overlay_enabled"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(
        summary_payload["appearance"]["background_overlay_opacity"],
        serde_json::json!(HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT)
    );

    let overlay = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/appearance/background-overlay")
                .header("x-elastos-home-token", admin.system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true,"opacity":0.42}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overlay.status(), StatusCode::OK);
    let overlay_body = axum::body::to_bytes(overlay.into_body(), usize::MAX)
        .await
        .unwrap();
    let overlay_payload: serde_json::Value = serde_json::from_slice(&overlay_body).unwrap();
    assert_eq!(
        overlay_payload["background_overlay_enabled"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        overlay_payload["background_overlay_opacity"],
        serde_json::json!(0.42)
    );

    let guest_summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", guest.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_summary.status(), StatusCode::OK);
    let guest_summary_body = axum::body::to_bytes(guest_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let guest_summary_payload: serde_json::Value =
        serde_json::from_slice(&guest_summary_body).unwrap();
    assert!(guest_summary_payload["appearance"]["background_image_url"].is_null());
    assert_eq!(
        guest_summary_payload["appearance"]["background_overlay_enabled"],
        serde_json::Value::Bool(false)
    );

    let guest_updated = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", guest.system_token.clone())
                .header(CONTENT_TYPE, "image/jpeg")
                .body(Body::from("guest-image"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_updated.status(), StatusCode::OK);

    let image = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", admin.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(image.status(), StatusCode::OK);
    assert_eq!(
        image
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/png")
    );
    let image_body = axum::body::to_bytes(image.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&image_body[..], b"admin-image");

    let guest_image = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", guest.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_image.status(), StatusCode::OK);
    assert_eq!(
        guest_image
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );
    let guest_image_body = axum::body::to_bytes(guest_image.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&guest_image_body[..], b"guest-image");

    let admin_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/Appearance/background-image.png",
            admin_protection.localhost_root
        ),
    )
    .unwrap();
    let admin_stored = std::fs::read_to_string(&admin_path).unwrap();
    assert!(!admin_stored.contains("admin-image"));
    assert!(admin_stored.contains("elastos.principal-root.object/v1"));
    assert!(admin_stored.contains(&admin_protection.localhost_root));

    let guest_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/Appearance/background-image.jpg",
            guest_protection.localhost_root
        ),
    )
    .unwrap();
    let guest_stored = std::fs::read_to_string(&guest_path).unwrap();
    assert!(!guest_stored.contains("guest-image"));
    assert!(guest_stored.contains("elastos.principal-root.object/v1"));
    assert!(guest_stored.contains(&guest_protection.localhost_root));

    let oversized = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", admin.system_token.clone())
                .header(CONTENT_TYPE, "image/png")
                .body(Body::from(vec![0_u8; HOME_BACKGROUND_IMAGE_MAX_BYTES + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
    let oversized_body = axum::body::to_bytes(oversized.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&oversized_body).unwrap(),
        "background image is larger than 5 MB"
    );

    let reset = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("DELETE")
                .uri("/api/apps/system/appearance/background-image")
                .header("x-elastos-home-token", admin.system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::OK);
    let reset_body = axum::body::to_bytes(reset.into_body(), usize::MAX)
        .await
        .unwrap();
    let reset_payload: serde_json::Value = serde_json::from_slice(&reset_body).unwrap();
    assert!(reset_payload["background_image_url"].is_null());

    let missing_image = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", admin.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_image.status(), StatusCode::NOT_FOUND);

    let guest_image_after_admin_reset = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/appearance/background-image")
                .header("x-elastos-home-token", guest.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_image_after_admin_reset.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_home_runtime_ensure_reuses_running_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "home-peer").await;

    let app = gateway_router(test_state(dir.path()));
    let resp = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/runtime/ensure")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["running"], true);
    assert_eq!(payload["version"], env!("ELASTOS_VERSION"));
    assert!(payload["note"].is_null());
    assert_eq!(payload["running_capsules"], json!([]));
}

#[tokio::test]
async fn test_system_summary_reports_identity_and_app_id() {
    let dir = tempfile::tempdir().unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["identity"]["profile"].is_null());
    assert_eq!(payload["identity"]["profile_setup_display_name"], "anders");
    assert!(payload["identity"]["device_did"].is_string());
    assert_eq!(payload["home"]["id"], "home");
    assert_eq!(payload["home"]["route"], "/apps/home/");
    assert_eq!(payload["app"]["id"], "system");
    assert_eq!(payload["app"]["route"], "/apps/system/");
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["runtime"]["version"], env!("ELASTOS_VERSION"));
    assert_eq!(payload["source"]["configured"], false);
    assert_eq!(payload["source"]["channel"], "not configured");
    assert_eq!(payload["source"]["installed_version"], "unknown");
    assert_eq!(
        payload["source"]["runtime_version"],
        env!("ELASTOS_VERSION")
    );
    assert_eq!(payload["source"]["update_checks_allowed"], false);
    assert!(payload["source"]["update_policy"]
        .as_str()
        .unwrap()
        .contains("No trusted source configured"));
    assert!(payload.get("storage").is_none());
    assert!(payload.get("webspace").is_none());
    assert!(payload.get("instance").is_none());
    assert_eq!(payload["runtime_log"]["available"], false);
}

#[tokio::test]
async fn test_system_summary_reports_trusted_source_update_policy() {
    let dir = tempfile::tempdir().unwrap();
    save_trusted_sources(
        dir.path(),
        &TrustedSourcesConfig {
            schema: "elastos.trusted-sources/v1".to_string(),
            default_source: "seed-node-linux".to_string(),
            sources: vec![TrustedSource {
                name: "seed-node-linux".to_string(),
                publisher_dids: vec!["did:key:seedpublisher".to_string()],
                channel: "canary".to_string(),
                discovery_uri: "elastos://source/did:key:seedpublisher/canary".to_string(),
                connect_ticket: "secret-ticket-must-not-render".to_string(),
                gateways: vec!["https://seed.example".to_string()],
                install_path: "/opt/elastos/bin/elastos".to_string(),
                installed_version: "0.5.0-dev".to_string(),
                head_cid: "bafyseedhead".to_string(),
                publisher_node_id: "seed-node-peer-id".to_string(),
                ipns_name: "k51seed".to_string(),
            }],
        },
    )
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("anders"));
    let resp = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["source"]["configured"], true);
    assert_eq!(payload["source"]["name"], "seed-node-linux");
    assert_eq!(payload["source"]["channel"], "canary");
    assert_eq!(payload["source"]["installed_version"], "0.5.0-dev");
    assert_eq!(
        payload["source"]["runtime_version"],
        env!("ELASTOS_VERSION")
    );
    assert_eq!(payload["source"]["source_peer"], "seed-node-peer-id");
    assert!(payload["source"]["transport"]
        .as_str()
        .unwrap()
        .contains("Carrier-first trusted source"));
    assert!(!serde_json::to_string(&payload["source"])
        .unwrap()
        .contains("secret-ticket-must-not-render"));
    if env!("ELASTOS_VERSION").contains("dev") {
        assert_eq!(payload["source"]["mode"], "development");
        assert_eq!(payload["source"]["update_checks_allowed"], false);
    } else {
        assert_eq!(payload["source"]["mode"], "review");
        assert_eq!(payload["source"]["update_checks_allowed"], true);
    }
}

#[tokio::test]
async fn test_system_guest_registration_requires_admin_passkey() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let local_system_token = system_app_token(dir.path());
    let authority = passkey_authority(dir.path());
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );

    let denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/access/guest-registration")
                .header("x-elastos-home-token", local_system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let guest_denied = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/access/guest-registration")
                .header("x-elastos-home-token", guest.system_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(guest_denied.status(), StatusCode::FORBIDDEN);

    let enabled = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/system/access/guest-registration")
                .header("x-elastos-home-token", authority.system_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let body = axum::body::to_bytes(enabled.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["role"], "admin");
    assert_eq!(payload["guest_registration_enabled"], true);

    let summary = app
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/system/summary")
                .header("x-elastos-home-token", authority.system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["access"]["role"], "admin");
    assert_eq!(payload["access"]["guest_registration_enabled"], true);
    assert!(payload["access"]["localhost_root"]
        .as_str()
        .unwrap()
        .starts_with("localhost://Users/"));
}

#[test]
fn system_runtime_activity_filters_attach_noise() {
    use elastos_runtime::primitives::audit::AuditEvent;

    let events = vec![
        AuditEvent::RuntimeStart {
            timestamp: elastos_common::SecureTimestamp::at(10),
            version: "0.1.2-dev".to_string(),
        },
        AuditEvent::SessionCreated {
            timestamp: elastos_common::SecureTimestamp::at(11),
            session_id: "s1".to_string(),
            session_type: "shell".to_string(),
            vm_id: None,
        },
        AuditEvent::PolicyProposal {
            timestamp: elastos_common::SecureTimestamp::at(12),
            request_id: "req-1".to_string(),
            recommended_outcome: "grant".to_string(),
            confidence: 0.9,
            rationale: "noise".to_string(),
        },
        AuditEvent::SecurityWarning {
            timestamp: elastos_common::SecureTimestamp::at(13),
            warning_type: "provider_offline".to_string(),
            details: "localhost-provider missing".to_string(),
        },
        AuditEvent::CapabilityDenied {
            timestamp: elastos_common::SecureTimestamp::at(14),
            request_id: "req-2".to_string(),
            session_id: "s2".to_string(),
            reason: "denied by shell".to_string(),
        },
    ];

    let summaries = system_runtime_activity_summaries(events);
    let rendered = summaries
        .iter()
        .map(|event| event.summary.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        vec![
            "Capability denied — denied by shell",
            "Security warning — provider_offline: localhost-provider missing",
            "Runtime started (0.1.2-dev)",
        ]
    );
}

#[tokio::test]
async fn test_removed_system_identity_mutations_cannot_succeed_or_mutate_state() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    for uri in [
        "/api/apps/system/identity/handle",
        "/api/apps/system/identity/profile-card",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"display_name":"anders"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::OK, "{uri}");
        assert!(
            matches!(
                response.status(),
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{uri}: {}",
            response.status()
        );
    }
    assert!(
        crate::collaboration_profile_authority::load_profile_authority(
            dir.path(),
            "person:local:missing",
            &crate::auth::principal_localhost_root("person:local:missing"),
        )
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn test_people_profile_update_rejects_proofless_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let update = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/profile")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", people_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"anders"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("proof-bound passkey session required"));
    assert!(elastos_identity::load_nickname(dir.path())
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_people_profile_update_creates_signed_profile_under_passkey_principal_authority() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "principal-handle-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Anders"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["identity"]["profile_setup_display_name"], "Anders");
    assert!(payload["identity"]["profile"].is_null());
    assert_eq!(
        payload["identity"]["profile_readiness"],
        serde_json::json!({
            "schema": "elastos.profile.readiness/v1",
            "status": "setup_required",
        })
    );

    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/profile")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"Anders Admin"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["profile"]["schema"], "elastos.profile-summary/v1");
    assert_eq!(payload["profile"]["display_name"], "Anders Admin");
    assert_eq!(payload["profile_readiness"]["status"], "ready");
    assert!(payload["profile"]["handle"].is_null());
    assert!(payload["profile"].get("profile_did").is_none());
    assert!(payload["profile"].get("previous_profile_sha256").is_none());
    assert!(payload["profile"].get("revision").is_none());
    assert!(payload["profile"].get("updated_at").is_none());
    assert!(payload["profile"].get("signature").is_none());
    assert!(payload["profile"].get("signer_did").is_none());
    assert!(payload["profile"].get("collaboration_endpoint").is_none());
    assert!(payload["profile"].get("collaboration_signers").is_none());
    assert!(elastos_identity::load_nickname(dir.path())
        .unwrap()
        .is_none());
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    assert_eq!(principal.display_name, "Anders");

    let summary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["identity"]["profile"]["display_name"],
        "Anders Admin"
    );
    assert_eq!(payload["identity"]["profile_readiness"]["status"], "ready");
    assert!(payload["identity"]["profile_setup_display_name"].is_null());
    assert!(payload["identity"]["profile"].get("signature").is_none());
    assert!(payload["identity"]["profile"].get("signer_did").is_none());
    assert!(payload["identity"]["profile"]
        .get("collaboration_endpoint")
        .is_none());
    assert!(payload["identity"]["profile"]
        .get("collaboration_signers")
        .is_none());

    let chat_launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(chat_launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let chat_token = launch_token_from_route(payload["route"].as_str().unwrap()).unwrap();

    let chat_session = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/chat-room/session/start")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", chat_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(chat_session.status(), StatusCode::OK);
    let body = axum::body::to_bytes(chat_session.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["display_name"], "Anders Admin");

    let restarted_app = gateway_router(test_state(dir.path()));
    let (status, payload) = home_test_get_json(
        &restarted_app,
        "/api/apps/home/summary",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["identity"]["profile_readiness"]["status"], "ready");
    assert_eq!(
        payload["identity"]["profile"]["display_name"],
        "Anders Admin"
    );
}

#[tokio::test]
async fn test_people_profile_update_uses_people_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "people-profile-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Anders"));
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);

    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/people/profile")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"display_name":"People Name"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let body = axum::body::to_bytes(update.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["profile"]["display_name"], "People Name");

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/home/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["identity"]["profile"]["display_name"],
        "People Name"
    );
}

#[tokio::test]
async fn test_home_and_people_summary_without_profile_are_side_effect_free() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Alex"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let profile_path =
        crate::collaboration_profile_authority::profile_authority_path(dir.path(), &localhost_root)
            .unwrap();
    let before = file_snapshot(dir.path());
    let before_identity: BTreeMap<_, _> = before
        .iter()
        .filter(|(path, _)| path.starts_with("identity/"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();

    let (home_status, home_payload) = home_test_get_json(
        &app,
        "/api/apps/home/summary",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(home_status, StatusCode::OK);
    assert!(home_payload["identity"]["profile"].is_null());
    assert_eq!(
        home_payload["identity"]["profile_readiness"]["status"],
        "setup_required"
    );
    assert_eq!(
        home_payload["identity"]["profile_setup_display_name"],
        "Alex"
    );

    let (people_status, people_payload) = home_test_get_json(
        &app,
        "/api/apps/people/summary",
        &authority.people_token,
        "null",
    )
    .await;
    assert_eq!(people_status, StatusCode::OK);
    assert!(people_payload["identity"]["profile"].is_null());
    assert_eq!(
        people_payload["identity"]["profile_readiness"]["status"],
        "setup_required"
    );
    assert_eq!(
        people_payload["identity"]["profile_setup_display_name"],
        "Alex"
    );

    let after = file_snapshot(dir.path());
    let after_identity: BTreeMap<_, _> = after
        .iter()
        .filter(|(path, _)| path.starts_with("identity/"))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect();

    assert_eq!(after, before);
    assert_eq!(after_identity, before_identity);
    assert!(!profile_path.exists());
    assert!(!profile_path.parent().unwrap().exists());
}

#[tokio::test]
async fn invalid_profile_projects_unavailable_without_mutating_or_claiming_setup() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Alex"));
    let principal =
        crate::auth::load_principal_for_proof_binding(dir.path(), &authority.proof_binding_id)
            .unwrap();
    crate::auth::store_test_principal_root_protection(dir.path(), &principal.principal_id);
    let object_uri = crate::collaboration_profile_authority::profile_authority_object_uri(
        &principal.localhost_root,
    );
    let profile_path = crate::collaboration_profile_authority::profile_authority_path(
        dir.path(),
        &principal.localhost_root,
    )
    .unwrap();
    crate::auth::write_protected_principal_root_object(
        dir.path(),
        &principal.principal_id,
        &principal.localhost_root,
        &object_uri,
        &profile_path,
        b"invalid profile authority",
    )
    .unwrap();
    let before = file_snapshot(dir.path());

    for (path, token, origin) in [
        (
            "/api/apps/home/summary",
            authority.home_token.as_str(),
            "http://localhost:61180",
        ),
        (
            "/api/apps/people/summary",
            authority.people_token.as_str(),
            "null",
        ),
    ] {
        let (status, payload) = home_test_get_json(&app, path, token, origin).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(
            payload["identity"]["profile_readiness"],
            serde_json::json!({
                "schema": "elastos.profile.readiness/v1",
                "status": "unavailable",
            }),
            "{path}"
        );
        assert!(payload["identity"]["profile"].is_null(), "{path}");
        assert!(
            payload["identity"]["profile_setup_display_name"].is_null(),
            "{path}"
        );
    }

    assert_eq!(file_snapshot(dir.path()), before);
}

#[tokio::test]
async fn authenticated_home_people_inbox_and_realtime_reads_are_observationally_pure() {
    const NETWORK: &str = "gateway-read-purity";
    let now = crate::auth::now_ts();
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let authority = passkey_authority_with_profile(dir.path(), "Local Profile");
    let inbox_token = app_token_for_authority(dir.path(), INBOX_CAPSULE_ID, &authority);
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let local_profile = crate::collaboration_profile_authority::load_profile_authority(
        dir.path(),
        &authority.principal_id,
        &localhost_root,
    )
    .unwrap()
    .unwrap();
    let local_seed: [u8; 32] = std::fs::read(dir.path().join("identity/device.key"))
        .unwrap()
        .try_into()
        .unwrap();
    let (local_device_key, local_device_did) = elastos_identity::derive_did(&local_seed);
    let local_device_key = SigningKey::from_bytes(&local_device_key.to_bytes());

    let (trusted_key, _) = generate_keypair();
    let network_profile = configured_discovery_network_profile_for_test(&trusted_key, NETWORK);
    let discovery_service =
        crate::collaboration_discovery_runtime::CollaborationDiscoveryService::new(
            SigningKey::from_bytes(&local_device_key.to_bytes()),
            network_profile.clone(),
            Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
        )
        .await
        .unwrap();
    let store = crate::collaboration_contact_store::CollaborationContactStore::new(
        dir.path(),
        &authority.principal_id,
        &localhost_root,
        network_profile,
        &local_profile,
        &local_device_did,
    )
    .unwrap();
    let local_advertisement = signed_discovery_message_for_test(
        &local_device_key,
        &local_profile.document().profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Conversation,
            id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationDiscoveryAdvertisementPayload {
                signed_profile: local_profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        now..now + crate::collaboration_discovery::COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
    );
    store
        .store_local_advertisement(&local_advertisement, now)
        .unwrap();

    let (remote_device_key, _) = generate_keypair();
    let remote_device_key = SigningKey::from_bytes(&remote_device_key.to_bytes());
    let remote_device_did = crate::crypto::encode_signing_key_did(&remote_device_key);
    let (remote_profile_key, _) = generate_keypair();
    let remote_profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
        &SigningKey::from_bytes(&remote_profile_key.to_bytes()),
        "Remote Profile",
        Some("remote"),
        1,
        None,
        now,
        vec![remote_device_did],
    )
    .unwrap();
    let request = signed_discovery_message_for_test(
        &remote_device_key,
        &remote_profile.document().profile_did,
        TestCollaborationMessageScope {
            network_id: NETWORK,
            conversation_id: crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_ID,
        },
        elastos_common::collaboration_protocol::CollaborationRecipient {
            kind: elastos_common::collaboration_protocol::CollaborationRecipientKind::Profile,
            id: local_profile.document().profile_did.clone(),
        },
        crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
        serde_json::to_value(
            crate::collaboration_discovery::CollaborationContactRequestPayload {
                advertisement_envelope_sha256:
                    elastos_common::collaboration_protocol::collaboration_message_envelope_sha256(
                        &local_advertisement,
                    ),
                signed_profile: remote_profile.signed_envelope().clone(),
            },
        )
        .unwrap(),
        (now + 1)
            ..(now
                + 1
                + crate::collaboration_discovery::COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS),
    );
    store
        .record_incoming_contact_request(&request, now + 1)
        .unwrap();

    // This released Services-only object is inert evidence until an explicit
    // authenticated launch owns its one-time rename.
    write_home_principal_object_json_for_authority(
        dir.path(),
        &authority,
        "people-contacts.json",
        json!({
            "schema": "elastos.people.contacts-state/v1",
            "principal_id": authority.principal_id,
            "localhost_root": localhost_root,
            "updated_at": 10,
            "contacts": {}
        }),
    );

    let mut state = test_state(dir.path());
    state.collaboration_discovery_service = Some(discovery_service.clone());
    let app = gateway_router(state);
    let legacy_uri = format!(
        "{}/.AppData/ElastOS/Home/people-contacts.json",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let current_uri = format!(
        "{}/.AppData/ElastOS/Home/services-peer-contacts.json",
        crate::auth::principal_localhost_root(&authority.principal_id)
    );
    let legacy_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &legacy_uri).unwrap();
    let current_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &current_uri).unwrap();
    let before_reads = file_snapshot(dir.path());
    let before_registrations = discovery_service.registered_context_snapshot_for_test();
    assert_eq!(
        before_registrations["sync"]
            .as_array()
            .expect("sync registration snapshot must be an array")
            .len(),
        0
    );
    let before_client_state = discovery_service.client_state_snapshot_for_test();

    let read_all = || async {
        let (home_status, home) = home_test_get_json(
            &app,
            "/api/apps/home/summary",
            &authority.home_token,
            "http://localhost:61180",
        )
        .await;
        assert_eq!(home_status, StatusCode::OK);
        let (people_status, people) = home_test_get_json(
            &app,
            "/api/apps/people/summary",
            &authority.people_token,
            "null",
        )
        .await;
        assert_eq!(people_status, StatusCode::OK);
        let (inbox_status, inbox) =
            home_test_get_json(&app, "/api/apps/inbox/summary", &inbox_token, "null").await;
        assert_eq!(inbox_status, StatusCode::OK);
        let (events_status, events) = home_test_get_json(
            &app,
            "/api/apps/home/events?wait_ms=0",
            &authority.home_token,
            "http://localhost:61180",
        )
        .await;
        assert_eq!(events_status, StatusCode::OK);
        (home, people, inbox, events)
    };

    let first = read_all().await;
    let second = read_all().await;
    assert_eq!(first.0["notifications"], second.0["notifications"]);
    assert_eq!(first.1["people"], second.1["people"]);
    assert_eq!(
        first.1["discovery"]["status"],
        second.1["discovery"]["status"]
    );
    assert_eq!(
        first.1["discovery"]["request_count"],
        second.1["discovery"]["request_count"]
    );
    assert_eq!(first.2["notifications"], second.2["notifications"]);
    assert_eq!(first.3["cursor"], second.3["cursor"]);
    for payload in [&first.0, &first.2] {
        let contact_requests = payload["notifications"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["kind"] == "contact_request")
            .count();
        assert_eq!(contact_requests, 1);
    }
    assert_eq!(first.1["discovery"]["request_count"], 1);
    assert_eq!(file_snapshot(dir.path()), before_reads);
    assert_eq!(
        discovery_service.registered_context_snapshot_for_test(),
        before_registrations
    );
    assert_eq!(
        discovery_service.client_state_snapshot_for_test(),
        before_client_state
    );
    assert!(legacy_path.is_file());
    assert!(!current_path.exists());

    let (launch_status, _) = home_test_post_json(
        &app,
        "/api/apps/home/launch",
        &authority.home_token,
        "http://localhost:61180",
        json!({ "target": INBOX_CAPSULE_ID }),
    )
    .await;
    assert_eq!(launch_status, StatusCode::OK);
    assert!(!legacy_path.exists());
    assert!(current_path.is_file());

    let after_launch = file_snapshot(dir.path());
    let after_launch_registrations = discovery_service.registered_context_snapshot_for_test();
    assert_eq!(
        after_launch_registrations["sync"]
            .as_array()
            .expect("sync registration snapshot must be an array")
            .len(),
        1
    );
    let after_launch_client_state = discovery_service.client_state_snapshot_for_test();
    let third = read_all().await;
    let fourth = read_all().await;
    assert_eq!(third.0["notifications"], fourth.0["notifications"]);
    assert_eq!(third.1["people"], fourth.1["people"]);
    assert_eq!(third.2["notifications"], fourth.2["notifications"]);
    assert_eq!(third.3["cursor"], fourth.3["cursor"]);
    assert_eq!(file_snapshot(dir.path()), after_launch);
    assert_eq!(
        discovery_service.registered_context_snapshot_for_test(),
        after_launch_registrations
    );
    assert_eq!(
        discovery_service.client_state_snapshot_for_test(),
        after_launch_client_state
    );
}

#[tokio::test]
async fn test_home_summary_ignores_old_profile_card_identity_file() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Alex"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let old_profile_card_uri =
        format!("{localhost_root}/.AppData/ElastOS/Profile/profile-card.json");
    let old_profile_card_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &old_profile_card_uri)
            .unwrap();
    crate::auth::write_protected_principal_root_object(
        dir.path(),
        &authority.principal_id,
        &localhost_root,
        &old_profile_card_uri,
        &old_profile_card_path,
        serde_json::to_string_pretty(&json!({
            "schema": "elastos.profile-card/v1",
            "profile_id": "did:key:old-profile-card",
            "display_name": "Legacy Name",
            "handle": "legacy",
            "updated_at": 1,
        }))
        .unwrap()
        .as_bytes(),
    )
    .unwrap();

    let (status, payload) = home_test_get_json(
        &app,
        "/api/apps/home/summary",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(payload["identity"]["profile"].is_null());
    assert_eq!(payload["identity"]["profile_setup_display_name"], "Alex");
}

#[tokio::test]
async fn test_home_summary_fails_closed_for_invalid_existing_device_key() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("Alex"));
    let request = test_browser_request("localhost:61180", "http://localhost:61180")
        .uri("/api/apps/home/summary")
        .header("x-elastos-home-token", authority.home_token.as_str())
        .body(Body::empty())
        .unwrap();
    let context =
        require_home_launch_token_context(dir.path(), request.headers(), HOME_CAPSULE_ID).unwrap();
    std::fs::create_dir_all(dir.path().join("identity")).unwrap();
    std::fs::write(dir.path().join("identity").join("device.key"), b"bad").unwrap();
    let err = match load_gateway_identity_summary_for_context(dir.path(), &context) {
        Ok(_) => panic!("invalid existing device.key should fail closed"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("device.key has invalid length"));
}

#[tokio::test]
async fn test_people_summary_requires_people_launch_token() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("Alex"));

    let rejected = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/apps/people/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/people/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.people_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["schema"], "elastos.people.summary/v1");
    assert!(payload["identity"]["profile"].is_null());
    assert_eq!(payload["identity"]["profile_setup_display_name"], "Alex");
    assert!(payload["people"]["contacts"].is_array());
    assert_eq!(
        payload["discovery"]["schema"],
        "elastos.people.discovery/v1"
    );
    assert_eq!(payload["discovery"]["configured"], false);
    assert_eq!(payload["discovery"]["status"], "unconfigured");
    assert!(payload["discovery"].get("peer_id").is_none());
}

#[tokio::test]
async fn test_home_launch_validates_shell_targets() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        "projection-app",
        "app",
        "Runtime-backed browser projection",
        None,
    );
    let projection_manifest_path = dir.path().join("capsules/projection-app/capsule.json");
    let mut projection_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&projection_manifest_path).unwrap()).unwrap();
    projection_manifest["runtime_abi"] = serde_json::json!("elastos.runtime-projection/v1");
    projection_manifest["bus_contract"] = serde_json::json!("elastos.runtime-projection/v1");
    projection_manifest["execution"] = serde_json::json!("web-projection");
    projection_manifest["projections"] = serde_json::json!(["web"]);
    std::fs::write(
        projection_manifest_path,
        serde_json::to_vec_pretty(&projection_manifest).unwrap(),
    )
    .unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "launch-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let home_token = home_app_token(dir.path());
    let cookie_only = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header(COOKIE, format!("{}={home_token}", HOME_SESSION_COOKIE))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_only.status(), StatusCode::FORBIDDEN);

    let ok = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", home_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let body = axum::body::to_bytes(ok.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "chat-room");
    assert_eq!(payload["target_kind"], "app");
    assert_eq!(payload["launch_status"], "launched");
    assert_eq!(payload["capsule_id"], "wasm-chat-room-instance");
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "chat-room");

    let library = app
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
                .body(Body::from(r#"{"target":"library"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(library.status(), StatusCode::OK);
    let body = axum::body::to_bytes(library.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "library");
    assert_eq!(payload["title"], "Library");
    assert_eq!(payload["target_kind"], "app");
    assert!(payload["launch_status"].is_null());
    assert!(payload["capsule_id"].is_null());
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "library");

    let projection = app
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
                .body(Body::from(r#"{"target":"projection-app"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(projection.status(), StatusCode::OK);
    let body = axum::body::to_bytes(projection.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "projection-app");
    assert!(payload["launch_status"].is_null());
    assert!(payload["capsule_id"].is_null());
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "projection-app");

    let hidden_connector = app
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
                .body(Body::from(r#"{"target":"wallet-metamask"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden_connector.status(), StatusCode::OK);
    let body = axum::body::to_bytes(hidden_connector.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "wallet-metamask");
    assert_eq!(payload["title"], "MetaMask");
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "wallet-metamask");

    let with_query = app
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
                    .body(Body::from(
                        r#"{"target":"documents","query":{"doc":"did:key:z6ExampleDoc","view":"read"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(with_query.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_query.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap_or_default();
    assert_isolated_launch_route(route, "documents");
    assert!(route.contains("doc=did%3Akey%3Az6ExampleDoc"), "{route}");
    assert!(route.contains("view=read"), "{route}");

    let with_elastos_uri = app
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
                    .body(Body::from(
                        r#"{"target":"documents","query":{"cid":"bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","uri":"elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","view":"read"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
    assert_eq!(with_elastos_uri.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_elastos_uri.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap_or_default();
    assert_isolated_launch_route(route, "documents");
    assert!(
        route.contains("cid=bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"),
        "{route}"
    );
    assert!(
        route.contains(
            "uri=elastos%3A%2F%2Fbafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        ),
        "{route}"
    );
    assert!(route.contains("view=read"), "{route}");

    let with_peer_invite = app
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
                .body(Body::from(
                    r#"{"target":"chat-room","query":{"invite":"elastos://peer/invite?token=abc-123"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_peer_invite.status(), StatusCode::OK);
    let body = axum::body::to_bytes(with_peer_invite.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let route = payload["route"].as_str().unwrap_or_default();
    assert_isolated_launch_route(route, "chat-room");
    assert!(
        route.contains("invite=elastos%3A%2F%2Fpeer%2Finvite%3Ftoken%3Dabc-123"),
        "{route}"
    );

    let viewer = app
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
    assert_eq!(viewer.status(), StatusCode::OK);
    let body = axum::body::to_bytes(viewer.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["target"], "gba-ucity");
    assert_eq!(payload["target_kind"], "object");
    let route = assert_isolated_launch_route(payload["route"].as_str().unwrap(), "gba-emulator");
    assert_eq!(
        route
            .query_pairs()
            .find(|(key, _)| key == "capsule")
            .unwrap()
            .1,
        "gba-ucity"
    );

    let missing = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", home_app_token(dir.path()))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"missing-shell-target"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_home_active_shell_uses_catalog_shell_candidates() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        "home-cli",
        "shell",
        "Home CLI",
        Some("<!doctype html><title>Home CLI</title>"),
    );
    write_test_browser_capsule(
        dir.path(),
        "regular-app",
        "app",
        "Regular app",
        Some("<!doctype html><title>Regular App</title>"),
    );
    write_test_browser_capsule(
        dir.path(),
        "manifest-shell",
        "shell",
        "Manifest shell",
        Some("<!doctype html><title>Manifest Shell</title>"),
    );
    let broken_shell_dir = dir.path().join("capsules").join("broken-shell");
    std::fs::create_dir_all(&broken_shell_dir).unwrap();
    std::fs::write(
        broken_shell_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": "broken-shell",
            "version": "0.1.0",
            "description": "No browser entrypoint",
            "author": "elastos",
            "role": "shell",
            "type": "wasm",
            "entrypoint": "broken-shell.wasm"
        }))
        .unwrap(),
    )
    .unwrap();

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let stale_state_uri = format!("{localhost_root}/.AppData/ElastOS/Home/active-shell.json");
    let stale_state_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &stale_state_uri).unwrap();
    write_home_principal_object_json_for_authority(
        dir.path(),
        &authority,
        "active-shell.json",
        json!({
            "schema": "elastos.home.active-shell/v1",
            "principal_id": authority.principal_id.clone(),
            "localhost_root": localhost_root.clone(),
            "active": "obsolete-shell"
        }),
    );

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["active_shell"]["schema"],
        "elastos.home.active-shell/v1"
    );
    assert_eq!(payload["active_shell"]["active"], HOME_GUI_SHELL_ID);
    let repaired_state = std::fs::read_to_string(&stale_state_path).unwrap();
    assert!(!repaired_state.contains("obsolete-shell"));
    assert!(repaired_state.contains(r#""active": "home-gui""#));
    let candidates = payload["active_shell"]["candidates"].as_array().unwrap();
    let candidate_names = candidates
        .iter()
        .map(|candidate| candidate["name"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        candidate_names,
        std::collections::BTreeSet::from([HOME_GUI_SHELL_ID, "home-cli"])
    );
    assert!(candidates
        .iter()
        .any(|candidate| candidate["name"] == HOME_GUI_SHELL_ID
            && candidate["role"] == "shell"
            && candidate["launchable"] == true
            && candidate["route"] == HOME_ROUTE));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == HOME_CAPSULE_ID));
    assert!(candidates
        .iter()
        .any(|candidate| candidate["name"] == "home-cli"
            && candidate["role"] == "shell"
            && candidate["launchable"] == true));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == "regular-app"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == "broken-shell"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate["name"] == "manifest-shell"));
    let manifest_shell_token = issue_home_launch_token(dir.path(), "manifest-shell").unwrap();
    let manifest_shell_update = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", manifest_shell_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(manifest_shell_update.status(), StatusCode::FORBIDDEN);
    let visible_targets = payload["targets"].as_array().unwrap();
    assert!(!visible_targets
        .iter()
        .any(|target| target["target"] == "home-cli"));
    assert!(visible_targets
        .iter()
        .any(|target| target["target"] == "regular-app"
            && target["role"] == "app"
            && target["target_kind"] == "app"));

    let app_rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"regular-app"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(app_rejected.status(), StatusCode::BAD_REQUEST);

    let cli_launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"home-cli"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cli_launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cli_launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let home_cli_token = launch_token_from_route(payload["route"].as_str().unwrap()).unwrap();

    let shell_summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shell_summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(shell_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], true);
    assert!(payload["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["target"] == "regular-app"));

    let catalog = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/catalog")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog.status(), StatusCode::OK);

    let interfaces = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/interfaces")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(interfaces.status(), StatusCode::OK);

    let esp_initialize = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/esp/initialize")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"esp_version":"0","accepts":["elastos.capsules.catalog/v1"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(esp_initialize.status(), StatusCode::OK);
    let body = axum::body::to_bytes(esp_initialize.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["protocol"], "elastos-shell-protocol");
    assert_eq!(payload["accepted"][0], "elastos.capsules.catalog/v1");

    let regular_launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"regular-app"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(regular_launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(regular_launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let regular_token = launch_token_from_route(payload["route"].as_str().unwrap()).unwrap();
    let catalog_rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/capsules/catalog")
                .header("x-elastos-home-token", regular_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog_rejected.status(), StatusCode::FORBIDDEN);

    let selected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-cli"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");

    let selected_from_shell = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_from_shell.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_from_shell.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");

    let selected_gui_from_shell = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", home_cli_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_gui_from_shell.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_gui_from_shell.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], HOME_GUI_SHELL_ID);
    let saved_state = std::fs::read_to_string(&stale_state_path).unwrap();
    let saved_state: serde_json::Value = serde_json::from_str(&saved_state).unwrap();
    assert_eq!(saved_state["active"], HOME_GUI_SHELL_ID);

    let selected_from_system = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_from_system.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_from_system.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], HOME_GUI_SHELL_ID);

    let selected_cli_from_system = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "null")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-cli"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected_cli_from_system.status(), StatusCode::OK);
    let body = axum::body::to_bytes(selected_cli_from_system.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active_shell"]["active"], "home-cli");

    let cookie_summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header(
                    COOKIE,
                    format!("{}={}", HOME_SESSION_COOKIE, authority.home_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cookie_summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["authority"]["signed_in"], true);
    assert_eq!(payload["active_shell"]["active"], "home-cli");

    let cookie_active_shell_write_rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header(
                    COOKIE,
                    format!("{}={}", HOME_SESSION_COOKIE, authority.home_token),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-gui"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        cookie_active_shell_write_rejected.status(),
        StatusCode::FORBIDDEN
    );

    let cookie_active_shell = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/active-shell")
                .header(
                    COOKIE,
                    format!("{}={}", HOME_SESSION_COOKIE, authority.home_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cookie_active_shell.status(), StatusCode::OK);
    let body = axum::body::to_bytes(cookie_active_shell.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], "home-cli");
}

#[tokio::test]
async fn test_home_shell_switch_preserves_runtime_facts_and_recovers_after_launch_failure() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path());
    std::fs::remove_file(dir.path().join("capsules/home-cli").join("home-cli.wasm")).unwrap();
    let app = gateway_router(state);
    let authority = passkey_authority_with_name(dir.path(), Some("shell-parity"));

    let (status, gui_before) = home_test_get_json(
        &app,
        "/api/apps/home/summary",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gui_before["active_shell"]["active"], HOME_GUI_SHELL_ID);
    let shared_before = home_shell_shared_facts(&gui_before);
    let installed_before = gui_before["capsule_catalog"]["capsules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|capsule| capsule["installed"] == true)
        .map(|capsule| capsule["name"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert!(installed_before.contains(HOME_GUI_SHELL_ID));
    assert!(installed_before.contains(HOME_CLI_CAPSULE_ID_FOR_TEST));

    let (status, failed_launch) = home_test_post_json(
        &app,
        "/api/apps/home/launch",
        &authority.home_token,
        "http://localhost:61180",
        json!({ "target": HOME_CLI_CAPSULE_ID_FOR_TEST }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(failed_launch["launch_status"], "failed");
    assert!(failed_launch["launch_detail"]
        .as_str()
        .unwrap()
        .contains("WASI Preview 1 product capsules are no longer materialized"));
    let cli_token = launch_token_from_route(failed_launch["route"].as_str().unwrap()).unwrap();

    let (status, cli_after_failure) =
        home_test_get_json(&app, "/api/apps/home/summary", &cli_token, "null").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        home_shell_shared_facts(&cli_after_failure),
        shared_before,
        "a failed shell launch changed shared Runtime facts"
    );
    assert_eq!(
        cli_after_failure["active_shell"]["active"], HOME_GUI_SHELL_ID,
        "a failed shell launch changed the active shell"
    );

    let (status, gui_catalog) = home_test_get_json(
        &app,
        "/api/capsules/catalog",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, cli_catalog) =
        home_test_get_json(&app, "/api/capsules/catalog", &cli_token, "null").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gui_catalog, cli_catalog);
    assert_eq!(gui_catalog, gui_before["capsule_catalog"]);

    let (status, gui_interfaces) = home_test_get_json(
        &app,
        "/api/capsules/interfaces",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, cli_interfaces) =
        home_test_get_json(&app, "/api/capsules/interfaces", &cli_token, "null").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gui_interfaces, cli_interfaces);
    assert_eq!(gui_interfaces, gui_before["capsule_interfaces"]);

    let (status, selected_cli) = home_test_post_json(
        &app,
        "/api/apps/home/active-shell",
        &cli_token,
        "null",
        json!({ "active": HOME_CLI_CAPSULE_ID_FOR_TEST }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selected_cli["active"], HOME_CLI_CAPSULE_ID_FOR_TEST);

    let (status, gui_after_switch) = home_test_get_json(
        &app,
        "/api/apps/home/summary",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, cli_after_switch) =
        home_test_get_json(&app, "/api/apps/home/summary", &cli_token, "null").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gui_after_switch["active_shell"]["active"], "home-cli");
    assert_eq!(cli_after_switch["active_shell"]["active"], "home-cli");
    assert_eq!(home_shell_shared_facts(&gui_after_switch), shared_before);
    assert_eq!(home_shell_shared_facts(&cli_after_switch), shared_before);
    let candidates = cli_after_switch["active_shell"]["candidates"]
        .as_array()
        .unwrap();
    let candidate_names = candidates
        .iter()
        .map(|candidate| candidate["name"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(candidate_names.len(), candidates.len());
    assert_eq!(
        candidate_names,
        BTreeSet::from([HOME_GUI_SHELL_ID, HOME_CLI_CAPSULE_ID_FOR_TEST])
    );
    assert!(cli_after_switch["runtime"]["running_capsules"]
        .as_array()
        .unwrap()
        .is_empty());

    let (status, selected_gui) = home_test_post_json(
        &app,
        "/api/apps/home/active-shell",
        &cli_token,
        "null",
        json!({ "active": HOME_GUI_SHELL_ID }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selected_gui["active"], HOME_GUI_SHELL_ID);
    let (status, gui_after_switchback) = home_test_get_json(
        &app,
        "/api/apps/home/summary",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        gui_after_switchback["active_shell"]["active"],
        HOME_GUI_SHELL_ID
    );
    assert_eq!(
        home_shell_shared_facts(&gui_after_switchback),
        shared_before
    );
}

#[tokio::test]
async fn test_home_active_shell_repairs_saved_home_state_but_rejects_home_updates() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        HOME_GUI_SHELL_ID,
        "shell",
        "Home GUI",
        Some("<!doctype html><title>Home GUI</title>"),
    );
    write_test_browser_capsule(
        dir.path(),
        "home-cli",
        "shell",
        "Home CLI",
        Some("<!doctype html><title>Home CLI</title>"),
    );

    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let state_uri = format!("{localhost_root}/.AppData/ElastOS/Home/active-shell.json");
    let state_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &state_uri).unwrap();
    write_home_principal_object_json_for_authority(
        dir.path(),
        &authority,
        "active-shell.json",
        json!({
            "schema": "elastos.home.active-shell/v1",
            "principal_id": authority.principal_id.clone(),
            "localhost_root": localhost_root.clone(),
            "active": HOME_CAPSULE_ID
        }),
    );

    let migrated = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(migrated.status(), StatusCode::OK);
    let body = axum::body::to_bytes(migrated.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["active"], HOME_GUI_SHELL_ID);
    assert!(payload["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |candidate| candidate["name"] == HOME_GUI_SHELL_ID && candidate["route"] == HOME_ROUTE
        ));
    assert!(!payload["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|candidate| candidate["name"] == HOME_CAPSULE_ID));
    let repaired_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(repaired_state["active"], HOME_GUI_SHELL_ID);

    let home_write_rejected = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home_write_rejected.status(), StatusCode::BAD_REQUEST);
    let saved_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(saved_state["active"], HOME_GUI_SHELL_ID);

    let invalid_update = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/active-shell")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"active":"home-old"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_update.status(), StatusCode::BAD_REQUEST);
    let saved_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(saved_state["active"], HOME_GUI_SHELL_ID);
}

#[tokio::test]
async fn test_home_browser_state_is_encrypted_for_protected_principal_root() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let app = gateway_router(test_state(dir.path()));

    let updated = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": false },
                        "session": { "openWindows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);

    let path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/browser-state.json",
            protection.localhost_root
        ),
    )
    .unwrap();
    let stored = std::fs::read_to_string(&path).unwrap();
    assert!(!stored.contains("desktopIconsVisible"));
    assert!(stored.contains("elastos.principal-root.object/v1"));
    assert!(stored.contains(&protection.localhost_root));

    let loaded = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_body = axum::body::to_bytes(loaded.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded_json: serde_json::Value = serde_json::from_slice(&loaded_body).unwrap();
    assert_eq!(
        loaded_json["layout"]["desktopIconsVisible"],
        serde_json::Value::Bool(false)
    );
}

#[tokio::test]
async fn test_home_browser_state_isolated_by_verified_principal() {
    let dir = tempfile::tempdir().unwrap();
    let admin = passkey_authority_with_name_role(
        dir.path(),
        Some("admin"),
        crate::auth::RuntimePrincipalRole::Admin,
    );
    let guest = passkey_authority_with_name_role(
        dir.path(),
        Some("guest"),
        crate::auth::RuntimePrincipalRole::Guest,
    );
    let admin_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &admin.principal_id);
    let guest_protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &guest.principal_id);
    let app = gateway_router(test_state(dir.path()));
    let context_id = "browser:000102030405060708090a0b0c0d0e0f";

    let (status, updated) = home_test_post_json(
        &app,
        "/api/apps/home/state",
        &admin.home_token,
        "http://localhost:61180",
        json!({
            "session": {
                "browser_context_id": context_id,
                "root_shell": "home-gui",
                "windows": [{ "target": "system", "active": true }]
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["principal_id"], admin.principal_id);
    assert_eq!(updated["session"]["browser_context_id"], context_id);

    let guest_state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/browser-state.json",
            guest_protection.localhost_root
        ),
    )
    .unwrap();
    assert!(!guest_state_path.exists());
    let (status, guest_state) = home_test_get_json(
        &app,
        "/api/apps/home/state",
        &guest.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(guest_state["principal_id"], guest.principal_id);
    assert!(guest_state["session"].is_null());
    assert!(!guest_state_path.exists());

    let (status, admin_state) = home_test_get_json(
        &app,
        "/api/apps/home/state",
        &admin.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(admin_state["principal_id"], admin.principal_id);
    assert_eq!(admin_state["session"]["browser_context_id"], context_id);
    let admin_state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/browser-state.json",
            admin_protection.localhost_root
        ),
    )
    .unwrap();
    assert!(admin_state_path.is_file());
}

#[tokio::test]
async fn test_home_browser_state_read_fallbacks_do_not_overwrite_protected_state() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let localhost_root = protection.localhost_root;
    let object_uri = format!("{localhost_root}/.AppData/ElastOS/Home/browser-state.json");
    let state_path =
        elastos_common::localhost::rooted_localhost_fs_path(dir.path(), &object_uri).unwrap();
    let app = gateway_router(test_state(dir.path()));

    assert!(!state_path.exists());
    let (status, missing) = home_test_get_json(
        &app,
        "/api/apps/home/state",
        &authority.home_token,
        "http://localhost:61180",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(missing["layout"].is_null());
    assert!(missing["session"].is_null());
    assert!(!state_path.exists());
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();

    let cases = [
        ("malformed", b"{not-json".to_vec()),
        (
            "stale-shape",
            serde_json::to_vec(&json!({
                "schema": "elastos.home.browser-state/v1",
                "localhost_root": localhost_root,
                "session": { "windows": [{ "target": "system" }] }
            }))
            .unwrap(),
        ),
        (
            "unsupported-schema",
            serde_json::to_vec(&json!({
                "schema": "elastos.home.browser-state/v0",
                "principal_id": authority.principal_id,
                "localhost_root": localhost_root,
                "session": { "windows": [{ "target": "system" }] }
            }))
            .unwrap(),
        ),
        (
            "principal-mismatch",
            serde_json::to_vec(&json!({
                "schema": "elastos.home.browser-state/v1",
                "principal_id": "principal:foreign",
                "localhost_root": localhost_root,
                "session": { "windows": [{ "target": "system" }] }
            }))
            .unwrap(),
        ),
        (
            "root-mismatch",
            serde_json::to_vec(&json!({
                "schema": "elastos.home.browser-state/v1",
                "principal_id": authority.principal_id,
                "localhost_root": "localhost://Users/foreign",
                "session": { "windows": [{ "target": "system" }] }
            }))
            .unwrap(),
        ),
    ];
    for (name, bytes) in cases {
        crate::auth::write_principal_root_object(
            dir.path(),
            &authority.principal_id,
            &localhost_root,
            &object_uri,
            &state_path,
            &bytes,
        )
        .unwrap();
        let protected_before = std::fs::read(&state_path).unwrap();
        let (status, fallback) = home_test_get_json(
            &app,
            "/api/apps/home/state",
            &authority.home_token,
            "http://localhost:61180",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert!(fallback["layout"].is_null(), "{name}");
        assert!(fallback["session"].is_null(), "{name}");
        assert!(
            fallback["recent_targets"].as_array().unwrap().is_empty(),
            "{name}"
        );
        assert_eq!(
            std::fs::read(&state_path).unwrap(),
            protected_before,
            "{name} read must not rewrite protected state"
        );
    }
}

#[tokio::test]
async fn test_home_browser_state_accepts_trusted_shells_only() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("shell-state"));
    let home_gui_token = authority.home_token.clone();
    let home_cli_token =
        app_token_for_authority(dir.path(), HOME_CLI_CAPSULE_ID_FOR_TEST, &authority);
    let system_token = authority.system_token.clone();
    let regular_token = launch_token_for_authority_context(dir.path(), "regular-app", &authority);
    let app = gateway_router(test_state(dir.path()));

    for (token, origin) in [
        (&home_gui_token, "http://localhost:61180"),
        (&home_cli_token, "null"),
    ] {
        let (status, updated) = home_test_post_json(
            &app,
            "/api/apps/home/state",
            token,
            origin,
            json!({
                "layout": { "desktopIconsVisible": false },
                "session": { "windows": [] },
                "recent_targets": ["system"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["layout"]["desktopIconsVisible"], false);

        let (status, loaded) =
            home_test_get_json(&app, "/api/apps/home/state", token, origin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(loaded["principal_id"], authority.principal_id);
    }

    for token in [&system_token, &regular_token] {
        let response = app
            .clone()
            .oneshot(
                test_browser_request("localhost:61180", "null")
                    .uri("/api/apps/home/state")
                    .header("x-elastos-home-token", token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn test_home_browser_state_drops_unknown_targets() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let desktop_object_entry = format!("object:{localhost_root}/Desktop/Test Folder");
    let trash_entry = format!("object:{localhost_root}/.Trash");
    let foreign_object_entry = "object:localhost://Users/foreign/Desktop/Bad".to_string();
    let mut layout = json!({
        "desktop": {
            "system": { "x": 12, "y": 12 },
            "people": { "x": 18, "y": 18 },
            "obsolete-wallet": { "x": 24, "y": 24 }
        },
        "desktopHidden": ["system", "people", "obsolete-wallet"],
        "desktopLabels": {
            "system": "System",
            "people": "People",
            "obsolete-wallet": "Old Wallet"
        },
        "taskbar": ["system", "people", "obsolete-wallet"],
        "desktopIconsVisible": true
    });
    {
        let desktop = layout["desktop"].as_object_mut().unwrap();
        desktop.insert(desktop_object_entry.clone(), json!({ "x": 36, "y": 36 }));
        desktop.insert(trash_entry.clone(), json!({ "x": 48, "y": 48 }));
        desktop.insert(foreign_object_entry.clone(), json!({ "x": 60, "y": 60 }));
    }
    let app = gateway_router(test_state(dir.path()));

    let updated = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": layout,
                        "session": {
                            "browser_context_id": "browser:test",
                            "windows": [
                                { "target": "obsolete-wallet", "active": true },
                                { "target": "people", "active": false },
                                { "target": "system", "active": false }
                            ]
                        },
                        "recent_targets": ["obsolete-wallet", "people", "system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let body = axum::body::to_bytes(updated.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["layout"]["desktop"].get("obsolete-wallet").is_none());
    assert!(json["layout"]["desktop"].get("people").is_some());
    assert!(json["layout"]["desktop"]
        .get(desktop_object_entry.as_str())
        .is_some());
    assert!(json["layout"]["desktop"]
        .get(trash_entry.as_str())
        .is_some());
    assert!(json["layout"]["desktop"]
        .get(foreign_object_entry.as_str())
        .is_none());
    assert!(json["layout"]["desktopLabels"]
        .get("obsolete-wallet")
        .is_none());
    assert!(json["layout"]["desktopLabels"].get("people").is_some());
    assert_eq!(json["layout"]["desktopHidden"], json!(["system", "people"]));
    assert_eq!(json["layout"]["taskbar"], json!(["system", "people"]));
    assert_eq!(json["session"]["windows"].as_array().unwrap().len(), 2);
    assert_eq!(json["session"]["windows"][0]["target"], "people");
    assert_eq!(json["session"]["windows"][1]["target"], "system");
    assert_eq!(json["recent_targets"], json!(["people", "system"]));
}

#[tokio::test]
async fn test_home_browser_state_recovers_from_malformed_saved_state() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    let state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!("{localhost_root}/.AppData/ElastOS/Home/browser-state.json"),
    )
    .unwrap();
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        &state_path,
        format!(
            "{}{}",
            serde_json::to_string_pretty(&json!({
                "schema": "elastos.home.browser-state/v1",
                "principal_id": authority.principal_id.clone(),
                "localhost_root": localhost_root.clone(),
                "layout": { "desktopIconsVisible": false },
                "session": { "windows": [] },
                "recent_targets": ["system"]
            }))
            .unwrap(),
            "}"
        ),
    )
    .unwrap();
    let malformed_before = std::fs::read(&state_path).unwrap();

    let app = gateway_router(test_state(dir.path()));
    let loaded = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_body = axum::body::to_bytes(loaded.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded_json: serde_json::Value = serde_json::from_slice(&loaded_body).unwrap();
    assert_eq!(
        loaded_json["principal_id"].as_str().unwrap(),
        authority.principal_id
    );
    assert!(loaded_json["layout"].is_null());
    assert!(loaded_json["recent_targets"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(&state_path).unwrap(), malformed_before);

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    assert_eq!(std::fs::read(&state_path).unwrap(), malformed_before);

    let updated = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": true },
                        "session": { "windows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap())
        .expect("Home should rewrite malformed browser state as valid JSON");
    assert_eq!(
        stored["layout"]["desktopIconsVisible"],
        serde_json::Value::Bool(true)
    );
}

#[tokio::test]
async fn test_home_browser_state_resets_plaintext_for_protected_principal_root() {
    let dir = tempfile::tempdir().unwrap();
    let authority = passkey_authority_with_name(dir.path(), Some("admin"));
    let protection =
        crate::auth::store_test_principal_root_protection(dir.path(), &authority.principal_id);
    let app = gateway_router(test_state(dir.path()));
    let state_path = elastos_common::localhost::rooted_localhost_fs_path(
        dir.path(),
        &format!(
            "{}/.AppData/ElastOS/Home/browser-state.json",
            protection.localhost_root
        ),
    )
    .unwrap();
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        &state_path,
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.home.browser-state/v1",
            "principal_id": authority.principal_id.clone(),
            "localhost_root": protection.localhost_root.clone(),
            "layout": { "desktopIconsVisible": false },
            "session": { "openWindows": [] },
            "recent_targets": ["system"]
        }))
        .unwrap(),
    )
    .unwrap();
    let plaintext_before = std::fs::read(&state_path).unwrap();

    let loaded = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(loaded.status(), StatusCode::OK);
    let loaded_body = axum::body::to_bytes(loaded.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded_json: serde_json::Value = serde_json::from_slice(&loaded_body).unwrap();
    assert_eq!(
        loaded_json["principal_id"].as_str().unwrap(),
        authority.principal_id
    );
    assert!(loaded_json["layout"].is_null());
    assert!(loaded_json["recent_targets"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(&state_path).unwrap(), plaintext_before);

    let summary = app
        .clone()
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .uri("/api/apps/home/summary")
                .header("x-elastos-home-token", authority.home_token.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    assert_eq!(std::fs::read(&state_path).unwrap(), plaintext_before);

    let updated = app
        .oneshot(
            test_browser_request("localhost:61180", "http://localhost:61180")
                .method("POST")
                .uri("/api/apps/home/state")
                .header("x-elastos-home-token", authority.home_token)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "layout": { "desktopIconsVisible": true },
                        "session": { "openWindows": [] },
                        "recent_targets": ["system"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let stored = std::fs::read_to_string(&state_path).unwrap();
    assert!(!stored.contains("desktopIconsVisible"));
    assert!(stored.contains("elastos.principal-root.object/v1"));
}

#[tokio::test]
async fn test_home_launch_starts_system_capsule_and_reports_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let _runtime = start_fake_runtime(dir.path(), bus, "system-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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
                .body(Body::from(r#"{"target":"system"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "system");
    assert_eq!(payload["target"], "system");
    assert_eq!(payload["target_kind"], "app");
    assert_eq!(payload["launch_status"], "launched");
    assert_eq!(payload["capsule_id"], "wasm-system-instance");
    let system_token = launch_token_from_route(payload["route"].as_str().unwrap()).unwrap();

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["runtime"]["running"], true);
    assert!(payload["runtime"]["note"].is_null());
    assert_eq!(payload["runtime_log"]["available"], true);
    assert!(payload["runtime_log"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "capsule_launch"));
}

#[tokio::test]
async fn test_home_launch_starts_chat_room_capsule_and_reports_runtime_activity() {
    let dir = tempfile::tempdir().unwrap();
    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "chat-room-peer").await;
    let app = gateway_router(test_state(dir.path()));
    let authority = passkey_authority(dir.path());

    let launch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/apps/home/launch")
                .header(HOST, "localhost:61180")
                .header("origin", "http://localhost:61180")
                .header("sec-fetch-site", "same-origin")
                .header("x-elastos-home-token", authority.home_token.as_str())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"target":"chat-room"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "chat-room");
    assert_eq!(payload["target"], "chat-room");
    assert_eq!(payload["target_kind"], "app");
    assert_eq!(payload["launch_status"], "launched");
    assert_eq!(payload["capsule_id"], "wasm-chat-room-instance");
    let launch_requests = runtime.launch_requests.lock().await;
    let launch_request = launch_requests.last().expect("runtime launch request");
    assert!(
        launch_request.get("principal_id").is_none(),
        "Home must not send raw principal_id authority to runtime launches"
    );
    let launch_grant = launch_request["launch_grant"]
        .as_str()
        .expect("runtime launch request includes signed launch_grant");
    let mut headers = HeaderMap::new();
    headers.insert("x-elastos-home-token", launch_grant.parse().unwrap());
    let grant_context = require_internal_shell_launch_grant_for_any_context(
        dir.path(),
        &headers,
        &[CHAT_ROOM_CAPSULE_ID],
    )
    .expect("runtime launch grant validates for chat-room");
    assert_eq!(grant_context.principal_id, authority.principal_id);

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", authority.system_token.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["runtime"]["running"],
        serde_json::Value::Bool(true),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(payload["runtime_log"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "capsule_launch"
            && event["summary"]
                .as_str()
                .unwrap_or_default()
                .contains("chat-room")));
}

#[tokio::test]
async fn test_home_launch_rejects_source_wasi_materialization() {
    let dir = tempfile::tempdir().unwrap();
    write_test_browser_capsule(
        dir.path(),
        "test-wasm-viewer",
        "app",
        "WASM test capsule",
        None,
    );
    let archive_dir = dir.path().join("capsules").join("test-wasm-viewer");
    let built_wasm = archive_dir
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join("test-wasm-viewer.wasm");
    std::fs::create_dir_all(built_wasm.parent().unwrap()).unwrap();
    std::fs::write(&built_wasm, b"\0asm").unwrap();
    assert!(!archive_dir.join("archive-manager.wasm").exists());

    let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
    let runtime = start_fake_runtime(dir.path(), bus, "archive-peer").await;
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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
                .body(Body::from(r#"{"target":"test-wasm-viewer"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["launch_status"], "failed");
    assert!(payload["launch_detail"]
        .as_str()
        .unwrap_or_default()
        .contains("WASI Preview 1 product capsules are no longer materialized"));

    let launch_requests = runtime.launch_requests.lock().await;
    assert!(
        launch_requests.is_empty(),
        "legacy WASI materialization must fail before Runtime launch"
    );
    assert!(
        !archive_dir.join("test-wasm-viewer.wasm").exists(),
        "source tree should not be dirtied with generated WASI artifacts"
    );
}

#[tokio::test]
async fn test_home_launch_reports_system_launch_failure_when_runtime_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let app = gateway_router(test_state(dir.path()));

    let launch = app
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
                .body(Body::from(r#"{"target":"system"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(launch.status(), StatusCode::OK);
    let body = axum::body::to_bytes(launch.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_isolated_launch_route(payload["route"].as_str().unwrap(), "system");
    assert_eq!(payload["launch_status"], "failed");
    assert!(payload["launch_detail"]
        .as_str()
        .unwrap()
        .contains("managed local runtime could not start"));
    let system_token = launch_token_from_route(payload["route"].as_str().unwrap()).unwrap();

    let summary = app
        .oneshot(
            Request::builder()
                .uri("/api/apps/system/summary")
                .header(HOST, "localhost:61180")
                .header("origin", "null")
                .header("x-elastos-home-token", system_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    let body = axum::body::to_bytes(summary.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["runtime"]["running"], false);
    assert_eq!(payload["runtime_log"]["available"], false);
    assert!(payload["runtime_log"]["note"]
        .as_str()
        .unwrap()
        .contains("Local runtime is not running"));
}

#[test]
fn resolve_capsule_dir_prefers_installed_capsule_before_dev_tree_copy() {
    let dir = tempfile::tempdir().unwrap();
    write_test_capsule_manifest(dir.path(), SYSTEM_CAPSULE_ID);

    let capsule_dir =
        resolve_capsule_dir(dir.path(), SYSTEM_CAPSULE_ID).expect("installed system capsule path");
    assert_eq!(
        capsule_dir,
        dir.path().join("capsules").join(SYSTEM_CAPSULE_ID)
    );
}

fn assert_rejects_unknown_gateway_field<T: serde::de::DeserializeOwned>(value: serde_json::Value) {
    let err = match serde_json::from_value::<T>(value) {
        Ok(_) => panic!("expected request body to reject unknown fields"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("unknown field"), "{err}");
}

#[test]
fn test_system_request_bodies_reject_hidden_authority_fields() {
    assert_rejects_unknown_gateway_field::<HomeBrowserStateUpdate>(json!({
        "session": null,
        "principal_id": "person:local:other"
    }));
    assert_rejects_unknown_gateway_field::<HomeActiveShellUpdate>(json!({
        "active": "home-gui",
        "route": "/apps/home/"
    }));
    assert_rejects_unknown_gateway_field::<PeopleProfileUpdateRequest>(json!({
        "display_name": "alice",
        "did": "did:elastos:alice"
    }));
    assert_rejects_unknown_gateway_field::<SystemBackgroundOverlayRequest>(json!({
        "enabled": true,
        "opacity": 0.25,
        "storage_path": "localhost://Users/self"
    }));
    assert_rejects_unknown_gateway_field::<SystemGuestRegistrationRequest>(json!({
        "enabled": true,
        "role": "admin"
    }));
}

#[test]
fn test_wallet_request_bodies_reject_hidden_authority_fields() {
    assert_rejects_unknown_gateway_field::<WalletApprovalRejectRequest>(json!({
        "reason": "no",
        "force": true
    }));
    assert_rejects_unknown_gateway_field::<WalletApprovalApproveRequest>(json!({
        "reason": "ok",
        "raw_signature": "0x00"
    }));
    assert_rejects_unknown_gateway_field::<WalletApprovalCompleteRequest>(json!({
        "payload_hash": "hash",
        "signature": "0xsig",
        "signer": "0xsigner",
        "private_key": "must-not-be-accepted"
    }));
    assert_rejects_unknown_gateway_field::<SystemWalletManagedCreateRequest>(json!({
        "chain_namespace": "eip155:20",
        "label": "Built-in",
        "seed_phrase": "must-not-be-accepted"
    }));
    assert_rejects_unknown_gateway_field::<SystemWalletDefaultRequest>(json!({
        "account_id": "account:test",
        "chain_namespace": "eip155:20",
        "intent": "personal_sign",
        "rpc_url": "https://example.invalid"
    }));
}

#[test]
fn test_home_and_inbox_request_bodies_reject_hidden_authority_fields() {
    assert_rejects_unknown_gateway_field::<HomeLaunchRequest>(json!({
        "target": "chat-room",
        "principal_id": "person:local:other"
    }));
    assert_rejects_unknown_gateway_field::<InboxActionRequest>(json!({
        "action_id": "wallet:test",
        "approve": true
    }));
}

#[test]
fn test_chat_request_bodies_reject_hidden_identity_fields() {
    assert_rejects_unknown_gateway_field::<RoomPollBody>(json!({
        "since": 1,
        "principal_id": "person:local:other"
    }));
    assert_rejects_unknown_gateway_field::<RoomSendBody>(json!({
        "request_id": "chat-message:test",
        "body": "hello",
        "sender_id": "did:key:forged"
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomAccessPolicyBody>(json!({
        "allow_guest_invites": true,
        "allow_member_invites": true,
        "allow_members_to_host_guests": false,
        "admin_override": true
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomMemberInviteBody>(json!({
        "member_did": "did:key:z6Mktest",
        "capability_token": "must-not-be-accepted"
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomMemberRemoveBody>(json!({
        "member_did": "did:key:z6Mktest",
        "delete_history": true
    }));
    assert_rejects_unknown_gateway_field::<ChatRoomInviteRevokeBody>(json!({
        "invite_id": "invite:test",
        "member_did": "did:key:z6Mktest"
    }));
    assert_rejects_unknown_gateway_field::<RoomUploadStartBody>(json!({
        "file_name": "note.md",
        "mime_type": "text/markdown",
        "size_bytes": 10,
        "ipfs_gateway": "https://example.invalid/ipfs"
    }));
}
