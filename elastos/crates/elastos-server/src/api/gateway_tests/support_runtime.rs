fn required_test_str<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, ProviderError> {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .ok_or_else(|| ProviderError::Provider(format!("missing {field}")))
}
fn required_test_string_array(
    value: &serde_json::Value,
    field: &str,
) -> Result<Vec<String>, ProviderError> {
    let values = value
        .get(field)
        .and_then(|value| value.as_array())
        .ok_or_else(|| ProviderError::Provider(format!("missing {field}")))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| ProviderError::Provider(format!("invalid {field}")))
        })
        .collect()
}

fn activate_test_capsule(data_dir: &std::path::Path, name: &str) {
    let path = data_dir.join("components.json");
    let mut components = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_else(|| {
            json!({
                "external": {},
                "capsules": {},
                "profiles": {}
            })
        });
    components["external"][name] = json!({
        "install_path": format!("capsules/{name}"),
        "platforms": {}
    });
    std::fs::write(path, serde_json::to_vec_pretty(&components).unwrap()).unwrap();
}

fn write_test_capsule_manifest(data_dir: &std::path::Path, name: &str) {
    let role = if name == GBA_EMULATOR_CAPSULE_ID {
        "viewer"
    } else {
        "app"
    };
    write_test_browser_capsule(data_dir, name, role, "Installed test capsule", None);
}

fn write_test_browser_capsule(
    data_dir: &std::path::Path,
    name: &str,
    role: &str,
    description: &str,
    index_html: Option<&str>,
) {
    activate_test_capsule(data_dir, name);
    let capsule_dir = data_dir.join("capsules").join(name);
    let browser_dir = capsule_dir.join("browser");
    std::fs::create_dir_all(&browser_dir).unwrap();
    let permissions = if name == GBA_EMULATOR_CAPSULE_ID {
        json!({
            "storage": ["localhost://Users/self/.AppData/LocalHost/GBA/*"]
        })
    } else {
        json!({})
    };
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": role,
            "type": "wasm",
            "entrypoint": format!("{name}.wasm"),
            "permissions": permissions
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        browser_dir.join("index.html"),
        index_html.unwrap_or("<!doctype html><title>Test Capsule</title>"),
    )
    .unwrap();
}

fn write_test_wasm_entrypoint(data_dir: &std::path::Path, name: &str) {
    std::fs::write(
        data_dir
            .join("capsules")
            .join(name)
            .join(format!("{name}.wasm")),
        b"\0asm",
    )
    .unwrap();
}

fn write_test_static_capsule(
    data_dir: &std::path::Path,
    name: &str,
    role: &str,
    description: &str,
    index_html: &str,
) {
    activate_test_capsule(data_dir, name);
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": role,
            "type": "data",
            "entrypoint": "index.html"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(capsule_dir.join("index.html"), index_html).unwrap();
}

fn write_test_provider_capsule(
    data_dir: &std::path::Path,
    name: &str,
    provides: &str,
    description: &str,
) {
    activate_test_capsule(data_dir, name);
    let capsule_dir = data_dir.join("capsules").join(name);
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": "provider",
            "type": "microvm",
            "entrypoint": name,
            "provides": provides,
            "authority": {
                "reason": "Test provider boundary",
                "capabilities": [{
                    "resource": provides,
                    "actions": ["read"],
                    "operations": ["status"]
                }],
                "audit_events": [format!("{name}.status")]
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

fn seed_test_browser_capsules(data_dir: &std::path::Path) {
    write_test_browser_capsule(
        data_dir,
        HOME_CAPSULE_ID,
        "shell",
        "Test Home capsule",
        Some(r#"<!doctype html><title>Home · ElastOS</title><script src="./home-shell-host.js"></script>"#),
    );
    std::fs::write(
        data_dir
            .join("capsules")
            .join(HOME_CAPSULE_ID)
            .join("browser")
            .join("home-shell-host.js"),
        "window.__TEST_HOME__ = true;",
    )
    .unwrap();

    for (name, role, description) in [
        ("home-gui", "shell", "Test Home GUI capsule"),
        ("home-cli", "shell", "Test Home CLI capsule"),
        ("assistant", "app", "Test Assistant capsule"),
        ("wallet", "app", "Test Wallet capsule"),
        ("browser", "app", "Test Browser capsule"),
        ("wallet-metamask", "app", "Test MetaMask connector"),
        ("wallet-unisat", "app", "Test UniSat connector"),
        (
            "wallet-walletconnect",
            "app",
            "Test WalletConnect connector",
        ),
    ] {
        write_test_browser_capsule(data_dir, name, role, description, None);
        write_test_wasm_entrypoint(data_dir, name);
    }
    write_test_provider_capsule(
        data_dir,
        "wallet-provider",
        "elastos://wallet/*",
        "Test Wallet provider",
    );

    write_test_browser_capsule(
        data_dir,
        SYSTEM_CAPSULE_ID,
        "app",
        "Test System capsule",
        Some("<!doctype html><title>System</title>"),
    );
    write_test_wasm_entrypoint(data_dir, SYSTEM_CAPSULE_ID);
    write_test_browser_capsule(
        data_dir,
        SERVICES_CAPSULE_ID,
        "app",
        "Test Services capsule",
        Some("<!doctype html><title>Services</title>"),
    );
    write_test_wasm_entrypoint(data_dir, SERVICES_CAPSULE_ID);
    write_test_browser_capsule(
        data_dir,
        PEOPLE_CAPSULE_ID,
        "app",
        "Test People capsule",
        Some("<!doctype html><title>People</title>"),
    );
    write_test_wasm_entrypoint(data_dir, PEOPLE_CAPSULE_ID);
    write_test_static_capsule(
        data_dir,
        DOCUMENTS_CAPSULE_ID,
        "app",
        "Test Documents capsule",
        "<!doctype html><title>Documents</title>",
    );
    write_test_static_capsule(
        data_dir,
        LIBRARY_CAPSULE_ID,
        "app",
        "Test Library capsule",
        "<!doctype html><title>Library</title>",
    );
    write_test_static_capsule(
        data_dir,
        INBOX_CAPSULE_ID,
        "app",
        "Test Inbox capsule",
        "<!doctype html><title>Inbox</title>",
    );
    write_test_browser_capsule(
        data_dir,
        CHAT_ROOM_CAPSULE_ID,
        "app",
        "Test Chat capsule",
        Some("<!doctype html><title>Chat</title>Chat"),
    );
    write_test_wasm_entrypoint(data_dir, CHAT_ROOM_CAPSULE_ID);
    std::fs::write(
        data_dir
            .join("capsules")
            .join(CHAT_ROOM_CAPSULE_ID)
            .join("browser")
            .join("chat_room_ui_bg.wasm"),
        b"\0asm",
    )
    .unwrap();

    write_test_browser_capsule(
        data_dir,
        GBA_EMULATOR_CAPSULE_ID,
        "viewer",
        "Test GBA emulator capsule",
        Some("<!doctype html><title>GBA Emulator</title>"),
    );
    write_test_wasm_entrypoint(data_dir, GBA_EMULATOR_CAPSULE_ID);
    write_test_viewer_capsule(
        data_dir,
        "gba-ucity",
        GBA_EMULATOR_CAPSULE_ID,
        "ucity.gba",
        "uCity",
    );
}

fn write_test_viewer_capsule(
    data_dir: &std::path::Path,
    name: &str,
    viewer: &str,
    entrypoint: &str,
    description: &str,
) {
    activate_test_capsule(data_dir, name);
    let capsule_dir = data_dir.join("capsules").join(name);
    let storage = if name == "gba-ucity" {
        "localhost://Users/self/.AppData/LocalHost/GBA/ucity/*"
    } else {
        "localhost://Users/self/.AppData/LocalHost/GBA/test/*"
    };
    std::fs::create_dir_all(&capsule_dir).unwrap();
    std::fs::write(
        capsule_dir.join("capsule.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "elastos.capsule/v1",
            "name": name,
            "version": "0.1.0",
            "description": description,
            "author": "elastos",
            "role": "content",
            "type": "data",
            "entrypoint": entrypoint,
            "viewer": viewer,
            "permissions": {
                "storage": [storage]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(capsule_dir.join(entrypoint), "rom-data").unwrap();
}

fn room_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .expect("room session cookie header")
}

fn browser_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix(&format!("{BROWSER_SESSION_COOKIE}="))
                .map(|_| value.split(';').next().unwrap_or_default().to_string())
        })
        .expect("browser session cookie header")
}

fn browser_request_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .strip_prefix("browser-session-request=")
                .map(|_| value.split(';').next().unwrap_or_default().to_string())
        })
        .expect("browser access request cookie header")
}

fn home_app_token(data_dir: &std::path::Path) -> String {
    issue_home_launch_token(data_dir, HOME_CAPSULE_ID).unwrap()
}

fn system_app_token(data_dir: &std::path::Path) -> String {
    issue_home_launch_token(data_dir, SYSTEM_CAPSULE_ID).unwrap()
}

fn people_app_token(data_dir: &std::path::Path) -> String {
    issue_home_launch_token(data_dir, PEOPLE_CAPSULE_ID).unwrap()
}

struct TestPasskeyAuthority {
    home_token: String,
    people_token: String,
    system_token: String,
    principal_id: String,
    proof_binding_id: String,
    session_id: String,
    grant_id: String,
}

fn passkey_authority(data_dir: &std::path::Path) -> TestPasskeyAuthority {
    passkey_authority_with_name(data_dir, None)
}

fn passkey_authority_with_name(
    data_dir: &std::path::Path,
    display_name: Option<&str>,
) -> TestPasskeyAuthority {
    passkey_authority_with_name_role(
        data_dir,
        display_name,
        crate::auth::RuntimePrincipalRole::Admin,
    )
}

/// Provisions the signed Profile authority a Chat room session requires.
///
/// Chat resolves its participant name from the signed Profile document, so a
/// fixture that expects session, join, or auth results must create the Profile
/// before calling those routes.
fn passkey_authority_with_profile(
    data_dir: &std::path::Path,
    display_name: &str,
) -> TestPasskeyAuthority {
    let authority = passkey_authority_with_name(data_dir, Some(display_name));
    provision_signed_profile(data_dir, &authority, display_name);
    authority
}

/// Creates the protected principal root and signed Profile document for an
/// already-provisioned passkey authority.
fn provision_signed_profile(
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
    display_name: &str,
) {
    let protection =
        crate::auth::store_test_principal_root_protection(data_dir, &authority.principal_id);
    let _ = elastos_identity::load_or_create_did(data_dir).unwrap();
    crate::collaboration_profile_authority::update_profile_authority(
        data_dir,
        &authority.principal_id,
        &protection.localhost_root,
        &authority.proof_binding_id,
        display_name,
        None,
        crate::auth::now_ts(),
    )
    .unwrap();
}

fn load_profile_for_authority(
    data_dir: &std::path::Path,
    authority: &TestPasskeyAuthority,
) -> crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
    let localhost_root = crate::auth::principal_localhost_root(&authority.principal_id);
    crate::collaboration_profile_authority::load_profile_authority(
        data_dir,
        &authority.principal_id,
        &localhost_root,
    )
    .unwrap()
    .unwrap()
}

fn test_signed_room_profile(
    data_dir: &std::path::Path,
    seed: u8,
    display_name: &str,
    handle: Option<&str>,
) -> (
    String,
    crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
) {
    let (_device_key, device_did) = elastos_identity::load_or_create_did(data_dir).unwrap();
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    let profile = crate::collaboration_profile_authority::signed_profile_document_for_test(
        &signing_key,
        display_name,
        handle,
        1,
        None,
        100 + seed as u64,
        vec![device_did.clone()],
    )
    .unwrap();
    (device_did, profile)
}

fn passkey_authority_with_name_role(
    data_dir: &std::path::Path,
    display_name: Option<&str>,
    role: crate::auth::RuntimePrincipalRole,
) -> TestPasskeyAuthority {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let now = crate::auth::now_ts();
    let credential_id = match role {
        crate::auth::RuntimePrincipalRole::Admin => "gateway-test-passkey",
        crate::auth::RuntimePrincipalRole::Guest => "gateway-test-guest-passkey",
    };
    let rp_id = "elastos.elacitylabs.com";
    let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
        credential_id: credential_id.to_string(),
        public_key: "gateway-test-public-key".to_string(),
        sign_count: 1,
        user_verified: true,
        origin: "https://elastos.elacitylabs.com".to_string(),
        rp_id: rp_id.to_string(),
        created_at: now,
        last_used_at: now,
        revoked_at: None,
    });
    let principal_id = crate::auth::passkey_credential_principal_id(rp_id, credential_id).unwrap();
    let principal = crate::auth::upsert_principal_for_binding_as_role_named(
        data_dir,
        binding,
        principal_id,
        role,
        display_name,
        now,
    )
    .unwrap();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", uuid_like_token()),
        session_id: format!("auth:{}", uuid_like_token()),
        principal_id: principal.principal_id.clone(),
        proof_binding_id: principal.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec![
            HOME_CAPSULE_ID.to_string(),
            PEOPLE_CAPSULE_ID.to_string(),
            SYSTEM_CAPSULE_ID.to_string(),
        ],
    };
    crate::auth::store_session_grant(data_dir, grant.clone()).unwrap();
    TestPasskeyAuthority {
        home_token: issue_home_launch_token_for_auth_grant(data_dir, HOME_CAPSULE_ID, &grant)
            .unwrap(),
        people_token: issue_home_launch_token_for_auth_grant(data_dir, PEOPLE_CAPSULE_ID, &grant)
            .unwrap(),
        system_token: issue_home_launch_token_for_auth_grant(data_dir, SYSTEM_CAPSULE_ID, &grant)
            .unwrap(),
        principal_id: principal.principal_id,
        proof_binding_id: principal.proof_binding_id,
        session_id: grant.session_id,
        grant_id: grant.grant_id,
    }
}

fn launch_token_for_authority_context(
    data_dir: &std::path::Path,
    app: &str,
    authority: &TestPasskeyAuthority,
) -> String {
    issue_home_launch_token_with_context(
        data_dir,
        app,
        &HomeLaunchTokenContext {
            principal_id: authority.principal_id.clone(),
            session_id: authority.session_id.clone(),
            proof_binding_id: Some(authority.proof_binding_id.clone()),
            grant_id: authority.grant_id.clone(),
        },
    )
    .unwrap()
}

fn projection_launch_token_for_authority_context(
    data_dir: &std::path::Path,
    app: &str,
    authority: &TestPasskeyAuthority,
) -> String {
    issue_home_projection_launch_token_with_context(
        data_dir,
        app,
        app,
        &HomeLaunchTokenContext {
            principal_id: authority.principal_id.clone(),
            session_id: authority.session_id.clone(),
            proof_binding_id: Some(authority.proof_binding_id.clone()),
            grant_id: authority.grant_id.clone(),
        },
    )
    .unwrap()
}

fn step_up_token_for_app_context(
    data_dir: &std::path::Path,
    app: &str,
    app_token: &str,
    operation: &str,
    request: &serde_json::Value,
) -> String {
    issue_passkey_step_up_token_for_test(data_dir, app_token, app, operation, request).unwrap()
}

fn stale_step_up_token_for_app_context(
    data_dir: &std::path::Path,
    app: &str,
    app_token: &str,
    operation: &str,
    request: &serde_json::Value,
) -> String {
    issue_passkey_step_up_token_at_for_test(
        data_dir,
        app_token,
        app,
        operation,
        request,
        crate::auth::now_ts().saturating_sub(181),
    )
    .unwrap()
}

fn app_token_for_authority(
    data_dir: &std::path::Path,
    app: &str,
    authority: &TestPasskeyAuthority,
) -> String {
    let now = crate::auth::now_ts();
    let grant = AuthSessionGrantV1 {
        schema: AuthSessionGrantV1::SCHEMA.to_string(),
        grant_id: format!("grant:{}", uuid_like_token()),
        session_id: format!("auth:{}", uuid_like_token()),
        principal_id: authority.principal_id.clone(),
        proof_binding_id: authority.proof_binding_id.clone(),
        issued_at: now,
        expires_at: now + 12 * 60 * 60,
        apps: vec![app.to_string()],
    };
    crate::auth::store_session_grant(data_dir, grant.clone()).unwrap();
    issue_home_launch_token_for_auth_grant(data_dir, app, &grant).unwrap()
}

fn runtime_wallet_authority_for_app_token(
    data_dir: &std::path::Path,
    app: &str,
    token: &str,
) -> RuntimeWalletAuthority {
    let mut headers = HeaderMap::new();
    headers.insert("host", HeaderValue::from_static("localhost:61180"));
    headers.insert("origin", HeaderValue::from_static("null"));
    headers.insert(
        "x-elastos-home-token",
        HeaderValue::from_str(token).unwrap(),
    );
    require_runtime_wallet_authority(data_dir, &headers, &[app]).unwrap()
}

fn evm_test_address(signing_key: &EvmSigningKey) -> String {
    let verifying_key = signing_key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    format!("0x{}", hex::encode(&digest[12..]))
}

fn evm_test_display_address(address: &str) -> String {
    elastos_runtime::auth::checksum_evm_address(address).unwrap()
}

fn evm_sign_message(signing_key: &EvmSigningKey, message: &str) -> String {
    let hash = elastos_runtime::auth::ethereum_signed_message_hash(message.as_bytes());
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(&hash)
        .expect("test EVM signature should be recoverable");
    let mut bytes = Vec::from(signature.to_bytes().as_slice());
    bytes.push(recovery_id.to_byte());
    format!("0x{}", hex::encode(bytes))
}

#[derive(Default)]
struct FakePeerBus {
    topic_members: HashMap<String, BTreeSet<String>>,
    topic_messages: HashMap<String, Vec<serde_json::Value>>,
    cursors: HashMap<(String, String, String), usize>,
    fail_message_substrings: Vec<String>,
    drop_remote_message_substrings: Vec<String>,
    local_only_message_substrings: Vec<String>,
}

#[derive(Clone)]
struct FakeRuntimeState {
    did: String,
    signing_key: ed25519_dalek::SigningKey,
    attach_secret: String,
    peer_id: String,
    bus: Arc<TokioMutex<FakePeerBus>>,
    audit_events: Arc<TokioMutex<Vec<elastos_runtime::primitives::audit::AuditEvent>>>,
    launch_requests: Arc<TokioMutex<Vec<serde_json::Value>>>,
    provider_requests: Arc<TokioMutex<Vec<serde_json::Value>>>,
    pending_capabilities: Arc<TokioMutex<HashMap<String, String>>>,
    capabilities_start_pending: bool,
}

struct FakeRuntimeHandle {
    _task: tokio::task::JoinHandle<()>,
    launch_requests: Arc<TokioMutex<Vec<serde_json::Value>>>,
}

fn verifying_key_from_did(did: &str) -> Option<ed25519_dalek::VerifyingKey> {
    let multibase = did.strip_prefix("did:key:z")?;
    let bytes = bs58::decode(multibase).into_vec().ok()?;
    if bytes.len() != 34 || bytes[0] != 0xed || bytes[1] != 0x01 {
        return None;
    }
    let key_bytes: [u8; 32] = bytes[2..34].try_into().ok()?;
    ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).ok()
}

async fn start_fake_runtime(
    data_dir: &std::path::Path,
    bus: Arc<TokioMutex<FakePeerBus>>,
    peer_id: &str,
) -> FakeRuntimeHandle {
    start_fake_runtime_configured(data_dir, data_dir, bus, peer_id, false).await
}

async fn start_fake_runtime_configured(
    data_dir: &std::path::Path,
    identity_dir: &std::path::Path,
    bus: Arc<TokioMutex<FakePeerBus>>,
    peer_id: &str,
    capabilities_start_pending: bool,
) -> FakeRuntimeHandle {
    let (signing_key, did) = elastos_identity::load_or_create_did(identity_dir).unwrap();
    let launch_requests = Arc::new(TokioMutex::new(Vec::new()));
    let provider_requests = Arc::new(TokioMutex::new(Vec::new()));
    let state = FakeRuntimeState {
        did,
        signing_key,
        attach_secret: format!("attach-{peer_id}"),
        peer_id: peer_id.to_string(),
        bus,
        audit_events: Arc::new(TokioMutex::new(vec![
            elastos_runtime::primitives::audit::AuditEvent::RuntimeStart {
                timestamp: elastos_common::SecureTimestamp::now(),
                version: env!("ELASTOS_VERSION").to_string(),
            },
        ])),
        launch_requests: launch_requests.clone(),
        provider_requests: provider_requests.clone(),
        pending_capabilities: Arc::new(TokioMutex::new(HashMap::new())),
        capabilities_start_pending,
    };
    let app = Router::new()
        .route("/api/auth/attach", post(fake_runtime_attach))
        .route("/api/health", get(fake_runtime_health))
        .route(
            "/api/capability/request",
            post(fake_runtime_capability_request),
        )
        .route(
            "/api/capability/request/:request_id",
            get(fake_runtime_capability_request_status),
        )
        .route(
            "/api/capsules",
            get(fake_runtime_list_capsules).post(fake_runtime_launch_capsule),
        )
        .route("/api/audit", get(fake_runtime_audit_log))
        .route("/api/provider/:scheme/:op", post(fake_runtime_provider))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let api_url = format!("http://{}", addr);
    let coords = json!({
        "api_url": api_url,
            "attach_secret": state.attach_secret,
            "pid": std::process::id(),
            "runtime_kind": crate::runtime_control::RUNTIME_KIND_OPERATOR,
            "binary_sha256": "",
            "policy_sha256": "",
    });
    let coords_json = serde_json::to_vec_pretty(&coords).unwrap();
    std::fs::write(data_dir.join("runtime-coords.json"), &coords_json).unwrap();
    std::fs::write(
        crate::runtime_control::home_runtime_coord_path(data_dir),
        coords_json,
    )
    .unwrap();
    FakeRuntimeHandle {
        _task: task,
        launch_requests,
    }
}

async fn fake_runtime_attach(
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    let secret = body
        .get("secret")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if secret != state.attach_secret {
        return (StatusCode::FORBIDDEN, "bad attach secret").into_response();
    }
    let scope = body
        .get("scope")
        .and_then(|value| value.as_str())
        .unwrap_or("client");
    AxumJson(json!({
        "token": format!("{scope}-{}", state.peer_id),
    }))
    .into_response()
}

async fn fake_runtime_health() -> Response {
    AxumJson(json!({ "version": env!("ELASTOS_VERSION") })).into_response()
}

fn fake_runtime_has_home_token(headers: &HeaderMap, state: &FakeRuntimeState) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == format!("Bearer shell-{}", state.peer_id))
        .unwrap_or(false)
}

async fn fake_runtime_list_capsules(
    headers: HeaderMap,
    AxumState(state): AxumState<FakeRuntimeState>,
) -> Response {
    if !fake_runtime_has_home_token(&headers, &state) {
        return (
            StatusCode::FORBIDDEN,
            "This endpoint requires shell privileges",
        )
            .into_response();
    }
    AxumJson(json!({ "capsules": [] })).into_response()
}

async fn fake_runtime_launch_capsule(
    headers: HeaderMap,
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    if !fake_runtime_has_home_token(&headers, &state) {
        return (
            StatusCode::FORBIDDEN,
            "This endpoint requires shell privileges",
        )
            .into_response();
    }
    state.launch_requests.lock().await.push(body.clone());
    let path = body
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let capsule_name = std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown");
    state.audit_events.lock().await.push(
        elastos_runtime::primitives::audit::AuditEvent::CapsuleLaunch {
            timestamp: elastos_common::SecureTimestamp::now(),
            capsule_id: format!("wasm-{}-instance", capsule_name),
            capsule_name: capsule_name.to_string(),
            cid: None,
            trust_level: elastos_runtime::primitives::audit::TrustLevel::Untrusted,
        },
    );
    AxumJson(json!({
        "id": format!("wasm-{}-instance", capsule_name),
        "name": capsule_name,
        "status": "running",
    }))
    .into_response()
}

async fn fake_runtime_audit_log(AxumState(state): AxumState<FakeRuntimeState>) -> Response {
    let events = state.audit_events.lock().await.clone();
    AxumJson(json!({
        "events": events,
        "total_in_memory": events.len(),
        "current_epoch": 0,
    }))
    .into_response()
}

async fn fake_runtime_capability_request(
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    let resource = body
        .get("resource")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let token = if resource.starts_with("elastos://did/") {
        format!("did-cap-{}", state.peer_id)
    } else if resource.starts_with("elastos://peer/") {
        format!("peer-cap-{}", state.peer_id)
    } else {
        format!("cap-{}", state.peer_id)
    };
    if state.capabilities_start_pending {
        let mut pending = state.pending_capabilities.lock().await;
        let request_id = format!("fake-capability-{}-{}", state.peer_id, pending.len() + 1);
        pending.insert(request_id.clone(), token);
        return AxumJson(json!({
            "status": "pending",
            "request_id": request_id,
        }))
        .into_response();
    }
    AxumJson(json!({ "token": token })).into_response()
}

async fn fake_runtime_capability_request_status(
    AxumPath(request_id): AxumPath<String>,
    AxumState(state): AxumState<FakeRuntimeState>,
) -> Response {
    let pending = state.pending_capabilities.lock().await;
    if let Some(token) = pending.get(&request_id) {
        return AxumJson(json!({
            "status": "granted",
            "token": token,
        }))
        .into_response();
    }
    AxumJson(json!({
        "status": "expired",
        "reason": "unknown fake capability request",
    }))
    .into_response()
}

async fn fake_runtime_provider(
    AxumPath((scheme, op)): AxumPath<(String, String)>,
    AxumState(state): AxumState<FakeRuntimeState>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Response {
    state.provider_requests.lock().await.push(json!({
        "scheme": scheme.clone(),
        "op": op.clone(),
        "body": body.clone(),
    }));
    match (scheme.as_str(), op.as_str()) {
        ("did", "get_did") => AxumJson(json!({
            "status": "ok",
            "data": { "did": state.did }
        }))
        .into_response(),
        ("did", "sign_chat_message") => {
            let sender_id = body
                .get("sender_id")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let ts = body.get("ts").and_then(|value| value.as_u64()).unwrap_or(0);
            let content = body
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let payload =
                elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
            let bytes = hex::decode(payload).expect("chat signing payload should be hex");
            let signature = state.signing_key.sign(&bytes);
            AxumJson(json!({
                "status": "ok",
                "data": { "signature": hex::encode(signature.to_bytes()) }
            }))
            .into_response()
        }
        ("did", "verify") => {
            let did = body
                .get("did")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let data = body
                .get("data")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let signature = body
                .get("signature")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let valid = {
                let Ok(bytes) = hex::decode(data) else {
                    return AxumJson(json!({"status":"error","message":"invalid hex payload"}))
                        .into_response();
                };
                let Ok(sig_bytes) = hex::decode(signature) else {
                    return AxumJson(json!({"status":"error","message":"invalid signature"}))
                        .into_response();
                };
                let Ok(sig) = ed25519_dalek::Signature::try_from(sig_bytes.as_slice()) else {
                    return AxumJson(json!({"status":"error","message":"invalid signature"}))
                        .into_response();
                };
                verifying_key_from_did(did)
                    .map(|key| key.verify(&bytes, &sig).is_ok())
                    .unwrap_or(false)
            };
            AxumJson(json!({
                "status": "ok",
                "data": { "valid": valid }
            }))
            .into_response()
        }
        ("peer", "get_ticket") => AxumJson(json!({
            "status": "ok",
            "data": {
                "ticket": format!("fake-ticket-{}", state.peer_id),
                "node_id": state.peer_id,
            }
        }))
        .into_response(),
        ("peer", "gossip_join") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut bus = state.bus.lock().await;
            let members = bus.topic_members.entry(topic.to_string()).or_default();
            if members.contains(&state.peer_id) {
                return AxumJson(json!({
                    "status": "error",
                    "code": "already_joined",
                    "message": "already joined"
                }))
                .into_response();
            }
            members.insert(state.peer_id.clone());
            AxumJson(json!({
                "status": "ok",
                "data": { "topic": topic }
            }))
            .into_response()
        }
        ("peer", "connect") | ("peer", "remember_peer") => {
            let ticket = body
                .get("ticket")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let peer = if ticket.trim().is_empty() {
                Vec::new()
            } else {
                vec!["trusted-source-peer".to_string()]
            };
            AxumJson(json!({
                "status": "ok",
                "data": {
                    "added": peer,
                    "connected": []
                }
            }))
            .into_response()
        }
        ("peer", "gossip_join_peers") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let peers = body
                .get("peers")
                .and_then(|value| value.as_array())
                .cloned()
                .unwrap_or_default();
            let mut bus = state.bus.lock().await;
            let members = bus.topic_members.entry(topic.to_string()).or_default();
            for peer in peers {
                if let Some(peer) = peer.as_str().map(str::trim).filter(|peer| !peer.is_empty()) {
                    members.insert(peer.to_string());
                }
            }
            AxumJson(json!({
                "status": "ok",
                "data": { "topic": topic }
            }))
            .into_response()
        }
        ("peer", "list_topic_peers") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let bus = state.bus.lock().await;
            let peers = bus
                .topic_members
                .get(topic)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|peer| peer != &state.peer_id)
                .collect::<Vec<_>>();
            AxumJson(json!({
                "status": "ok",
                "data": { "topic": topic, "peers": peers }
            }))
            .into_response()
        }
        ("peer", "gossip_send") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut bus = state.bus.lock().await;
            let peers = bus.topic_members.get(topic).cloned().unwrap_or_default();
            let remote_peer_count = peers.iter().filter(|peer| *peer != &state.peer_id).count();
            let message = body.get("message").and_then(|value| value.as_str()).unwrap_or("");
            let fail_index = bus
                .fail_message_substrings
                .iter()
                .position(|needle| message.contains(needle));
            if let Some(index) = fail_index {
                bus.fail_message_substrings.remove(index);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    AxumJson(json!({
                        "status": "error",
                        "message": "fake Carrier gossip_send failed"
                    })),
                )
                    .into_response();
            }
            let local_only = bus
                .local_only_message_substrings
                .iter()
                .any(|needle| message.contains(needle));
            let drop_index = (remote_peer_count > 0)
                .then(|| {
                    bus.drop_remote_message_substrings
                        .iter()
                        .position(|needle| message.contains(needle))
                })
                .flatten();
            if let Some(index) = drop_index {
                bus.drop_remote_message_substrings.remove(index);
            } else if !local_only {
                bus.topic_messages
                    .entry(topic.to_string())
                    .or_default()
                    .push(json!({
                        "sender_id": body.get("sender_id").cloned().unwrap_or(serde_json::Value::Null),
                        "sender_nick": body.get("sender").cloned().unwrap_or(serde_json::Value::Null),
                        "content": body.get("message").cloned().unwrap_or(serde_json::Value::Null),
                        "ts": body.get("ts").cloned().unwrap_or(serde_json::Value::from(0u64)),
                        "signature": body.get("signature").cloned().unwrap_or(serde_json::Value::Null),
                    }));
            }
            let mut response = json!({
                "status": "ok",
                "data": { "remote_peer_count": remote_peer_count }
            });
            if remote_peer_count == 0 || local_only {
                response["broadcast"] = serde_json::Value::String("local_only".to_string());
            }
            AxumJson(response).into_response()
        }
        ("peer", "gossip_recv") => {
            let topic = body
                .get("topic")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let consumer_id = body
                .get("consumer_id")
                .and_then(|value| value.as_str())
                .unwrap_or("default");
            let limit = body
                .get("limit")
                .and_then(|value| value.as_u64())
                .unwrap_or(50);
            let skip_sender_id = body
                .get("skip_sender_id")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut bus = state.bus.lock().await;
            let cursor_key = (
                state.peer_id.clone(),
                topic.to_string(),
                consumer_id.to_string(),
            );
            let start = *bus.cursors.get(&cursor_key).unwrap_or(&0);
            let all = bus.topic_messages.get(topic).cloned().unwrap_or_default();
            let count = all.len().saturating_sub(start).min(limit as usize);
            let selected = all
                .into_iter()
                .skip(start)
                .take(limit as usize)
                .filter(|message| {
                    skip_sender_id.is_empty()
                        || message
                            .get("sender_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            != skip_sender_id
                })
                .collect::<Vec<_>>();
            let next_cursor = start + count;
            bus.cursors.insert(cursor_key, next_cursor);
            AxumJson(json!({
                "status": "ok",
                "data": {
                    "messages": selected,
                    "scanned": count,
                    "limit": limit,
                    "next_cursor": next_cursor
                }
            }))
            .into_response()
        }
        _ => AxumJson(json!({
            "status": "error",
            "message": format!("unsupported fake runtime operation {scheme}/{op}"),
        }))
        .into_response(),
    }
}
