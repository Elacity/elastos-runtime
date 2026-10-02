use super::*;

fn copy_home(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            copy_home(&entry.path(), &target.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires Playwright and Chromium; isolated legacy security-key journey"]
async fn content_sandbox_browser_legacy_passkey_hint_preserves_existing_admin_and_refuses_invalid_proofs(
) {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let _auth = set_test_home_launch_auth_data_dir(dir.path());
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let home = dir.path().join("capsules/home/browser");
    copy_home(&repo.join("capsules/home/browser"), &home);
    std::fs::copy(
        repo.join("capsules/home/capsule.json"),
        home.parent().unwrap().join("capsule.json"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("components.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "external": { "home": { "install_path": "capsules/home", "platforms": {} } },
            "capsules": {},
            "profiles": {}
        }))
        .unwrap(),
    )
    .unwrap();
    // Exercise the production markup and authentication module, with a callback
    // that holds completion in the trusted test page instead of launching apps.
    std::fs::write(
        home.join("home-shell-host.js"),
        r#"
import { bindHomeUnlock, showHomeUnlock } from './shell-auth.js?v=home-20260802a';
bindHomeUnlock();
window.showLegacyUnlock = () => showHomeUnlock(response => { window.legacyCompletion = response; });
window.legacyReady = true;
"#,
    )
    .unwrap();
    let state = GatewayState {
        provider_registry: None,
        collaboration_chat_product_port: None,
        collaboration_presence_product_port: None,
        carrier_endpoint: None,
        collaboration_discovery_service: None,
        identity_manager: Arc::new(std::sync::OnceLock::new()),
        cache_dir: dir.path().to_path_buf(),
        data_dir: dir.path().to_path_buf(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let app = gateway_router_with_api_url(state, origin.clone());
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await
    });
    let mut command = tokio::process::Command::new("node");
    command
        .arg(repo.join("scripts/gateway-legacy-passkey-browser-smoke.mjs"))
        .env("ELASTOS_LEGACY_PASSKEY_URL", &origin)
        .env(
            "ELASTOS_LEGACY_PASSKEY_METADATA",
            crate::auth::auth_state_path(dir.path()).unwrap(),
        )
        .kill_on_drop(true);
    let result = tokio::time::timeout(std::time::Duration::from_secs(165), command.output()).await;
    let _ = shutdown_tx.send(());
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut server)
        .await
        .is_err()
    {
        server.abort();
        let _ = server.await;
        panic!("legacy security-key fixture gateway did not stop");
    }
    let output = result
        .expect("legacy security-key browser timeout")
        .expect("start browser fixture");
    assert!(
        output.status.success(),
        "legacy security-key journey failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
