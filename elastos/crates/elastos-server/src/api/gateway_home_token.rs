use axum::http::{HeaderMap, HeaderValue};
use base64::Engine as _;
use rand::RngCore;

use super::*;

#[derive(Clone)]
pub(crate) struct HomeLaunchTokenContext {
    pub principal_id: String,
    pub session_id: String,
    pub proof_binding_id: Option<String>,
    pub grant_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HomeLaunchTokenPayload {
    schema: String,
    app: String,
    iat: u64,
    exp: u64,
    principal_id: String,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_binding_id: Option<String>,
    grant_id: String,
    non_delegatable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent: Option<HomeLaunchTokenIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HomeLaunchTokenIntent {
    operation: String,
    request_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HomeLaunchTokenEnvelope {
    payload: HomeLaunchTokenPayload,
    signature: String,
    signer_did: String,
}

struct RequiredHomeLaunchToken {
    app: String,
    context: HomeLaunchTokenContext,
}

pub(crate) fn home_session_cookie_header_for_token(
    token: &str,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    home_launch_cookie_header(
        HOME_SESSION_COOKIE,
        token,
        HOME_LAUNCH_TOKEN_TTL_SECS,
        "/",
        secure,
    )
}

pub(crate) fn home_session_clear_cookie_header(secure: bool) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{}=; Max-Age=0; Path=/; HttpOnly; SameSite=Strict",
        HOME_SESSION_COOKIE
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

fn home_launch_cookie_header(
    name: &str,
    token: &str,
    max_age_secs: u64,
    path: &str,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value =
        format!("{name}={token}; Max-Age={max_age_secs}; Path={path}; HttpOnly; SameSite=Strict");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn local_home_launch_token_context(
    data_dir: &std::path::Path,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let (_signing_key, did) = elastos_identity::load_or_create_did(data_dir)?;
    Ok(HomeLaunchTokenContext {
        principal_id: elastos_runtime::auth::PrincipalId::device_did(&did)
            .as_str()
            .to_string(),
        session_id: format!("local:{}", uuid_like_token()),
        proof_binding_id: None,
        grant_id: format!("grant:local:{}", uuid_like_token()),
    })
}

pub(crate) fn uuid_like_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub(crate) fn issue_local_runtime_home_launch_token(
    data_dir: &std::path::Path,
    app: &str,
) -> anyhow::Result<String> {
    let context = local_home_launch_token_context(data_dir)?;
    issue_home_launch_token_with_context(data_dir, app, &context)
}

#[cfg(test)]
pub(crate) fn issue_home_launch_token(
    data_dir: &std::path::Path,
    app: &str,
) -> anyhow::Result<String> {
    let context = local_home_launch_token_context(data_dir)?;
    issue_home_launch_token_with_context(data_dir, app, &context)
}

pub(crate) fn issue_home_launch_token_for_auth_grant(
    data_dir: &std::path::Path,
    app: &str,
    grant: &elastos_runtime::auth::AuthSessionGrantV1,
) -> anyhow::Result<String> {
    if !grant.apps.iter().any(|allowed| allowed == app) {
        anyhow::bail!("auth session grant is not authorized for app");
    }
    issue_home_launch_token_with_context(
        data_dir,
        app,
        &HomeLaunchTokenContext {
            principal_id: grant.principal_id.clone(),
            session_id: grant.session_id.clone(),
            proof_binding_id: Some(grant.proof_binding_id.clone()),
            grant_id: grant.grant_id.clone(),
        },
    )
}

pub(crate) fn issue_home_launch_token_with_context(
    data_dir: &std::path::Path,
    app: &str,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<String> {
    issue_home_launch_token_with_context_and_intent(data_dir, app, context, None)
}

pub(crate) fn issue_home_launch_token_with_intent(
    data_dir: &std::path::Path,
    app: &str,
    context: &HomeLaunchTokenContext,
    operation: &str,
    request: &serde_json::Value,
) -> anyhow::Result<String> {
    let operation = operation.trim();
    if operation.is_empty() || operation.len() > 128 {
        anyhow::bail!("invalid Home authority operation");
    }
    let request_bytes = serde_json::to_vec(request)?;
    if request_bytes.len() > 64 * 1024 {
        anyhow::bail!("Home authority request is too large");
    }
    issue_home_launch_token_with_context_and_intent(
        data_dir,
        app,
        context,
        Some(HomeLaunchTokenIntent {
            operation: operation.to_string(),
            request_sha256: hex::encode(Sha256::digest(request_bytes)),
        }),
    )
}

fn issue_home_launch_token_with_context_and_intent(
    data_dir: &std::path::Path,
    app: &str,
    context: &HomeLaunchTokenContext,
    intent: Option<HomeLaunchTokenIntent>,
) -> anyhow::Result<String> {
    let (signing_key, _did) = elastos_identity::load_or_create_did(data_dir)?;
    let now = now_ts();
    let envelope = HomeLaunchTokenEnvelope {
        payload: HomeLaunchTokenPayload {
            schema: "elastos.home.launch-token/v2".to_string(),
            app: app.to_string(),
            iat: now,
            exp: now + HOME_LAUNCH_TOKEN_TTL_SECS,
            principal_id: context.principal_id.clone(),
            session_id: context.session_id.clone(),
            proof_binding_id: context.proof_binding_id.clone(),
            grant_id: context.grant_id.clone(),
            non_delegatable: true,
            intent,
        },
        signature: String::new(),
        signer_did: String::new(),
    };
    let canonical = serde_json::to_string(&serde_json::to_value(&envelope.payload)?)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &signing_key,
        HOME_LAUNCH_TOKEN_DOMAIN,
        canonical.as_bytes(),
    );
    let envelope = HomeLaunchTokenEnvelope {
        signature,
        signer_did,
        ..envelope
    };
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope)?))
}

pub(crate) fn require_home_launch_token(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    expected_app: &str,
) -> anyhow::Result<()> {
    require_home_launch_token_for_any(data_dir, headers, &[expected_app]).map(|_| ())
}

pub(crate) fn require_home_launch_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    expected_app: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_context(data_dir, headers, &[expected_app])
}

pub(crate) fn require_home_launch_token_for_any(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<String> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
        .map(|required| required.app)
}

pub(crate) fn require_home_launch_token_for_any_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
        .map(|required| required.context)
}

pub(crate) fn require_home_launch_token_for_any_app_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<(String, HomeLaunchTokenContext)> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
        .map(|required| (required.app, required.context))
}

pub(crate) fn consume_fresh_passkey_home_token(
    data_dir: &std::path::Path,
    token: &str,
    expected_context: &HomeLaunchTokenContext,
    expected_app: &str,
    max_age_secs: u64,
    operation: &str,
    request: &serde_json::Value,
) -> anyhow::Result<()> {
    let token = token.trim();
    if token.is_empty() {
        anyhow::bail!("missing fresh passkey token");
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| anyhow::anyhow!("invalid fresh passkey token encoding"))?;
    let envelope: HomeLaunchTokenEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid fresh passkey token payload"))?;
    if envelope.payload.schema != "elastos.home.launch-token/v2" {
        anyhow::bail!("unsupported fresh passkey token schema");
    }
    let local_did = load_existing_gateway_runtime_did(data_dir)
        .ok_or_else(|| anyhow::anyhow!("gateway identity is unavailable"))?;
    crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        HOME_LAUNCH_TOKEN_DOMAIN,
        &[local_did],
    )
    .map_err(|err| anyhow::anyhow!("invalid fresh passkey token: {}", err))?;
    let now = now_ts();
    if envelope.payload.app != expected_app {
        anyhow::bail!("fresh passkey token is not authorized for this operation");
    }
    if envelope.payload.principal_id != expected_context.principal_id {
        anyhow::bail!("fresh passkey token belongs to a different principal");
    }
    let Some(proof_binding_id) = envelope.payload.proof_binding_id.as_deref() else {
        anyhow::bail!("fresh passkey token is not proof-bound");
    };
    if !proof_binding_id.starts_with("proof:passkey:") {
        anyhow::bail!("fresh passkey verification is required");
    }
    if envelope.payload.exp <= now {
        anyhow::bail!("fresh passkey token expired");
    }
    if envelope.payload.iat > now.saturating_add(60)
        || now.saturating_sub(envelope.payload.iat) > max_age_secs
    {
        anyhow::bail!("fresh passkey token is too old");
    }
    let grant =
        crate::auth::load_active_session_grant(data_dir, &envelope.payload.session_id, now)?;
    if grant.principal_id != envelope.payload.principal_id
        || grant.proof_binding_id != proof_binding_id
        || grant.grant_id != envelope.payload.grant_id
        || envelope.payload.session_id != expected_context.session_id
        || envelope.payload.grant_id != expected_context.grant_id
        || expected_context.proof_binding_id.as_deref() != Some(proof_binding_id)
    {
        anyhow::bail!("fresh passkey token authority context mismatch");
    }
    let operation = operation.trim();
    if operation.is_empty() {
        anyhow::bail!("fresh passkey operation is required");
    }
    let request_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(request)?));
    let Some(intent) = envelope.payload.intent.as_ref() else {
        anyhow::bail!("fresh passkey token is not bound to an operation");
    };
    if intent.operation != operation || intent.request_sha256 != request_sha256 {
        anyhow::bail!("fresh passkey token intent mismatch");
    }
    let auth_state_path = crate::auth::auth_state_path(data_dir)?;
    let root = auth_state_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("authentication state root is unavailable"))?
        .join("consumed-passkey-proofs");
    std::fs::create_dir_all(&root)?;
    let token_sha256 = hex::encode(Sha256::digest(token.as_bytes()));
    let path = root.join(format!("{token_sha256}.json"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!("fresh passkey token has already been used")
            } else {
                anyhow::Error::from(err)
            }
        })?;
    let marker = serde_json::json!({
        "schema": "elastos.auth.consumed-passkey-proof/v1",
        "token_sha256": token_sha256,
        "principal_id": envelope.payload.principal_id,
        "session_id": envelope.payload.session_id,
        "app": envelope.payload.app,
        "operation": operation,
        "request_sha256": request_sha256,
        "consumed_at": now,
    });
    std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(&marker)?)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn require_home_token(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<()> {
    require_home_token_context(data_dir, headers).map(|_| ())
}

pub(crate) fn require_home_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_from(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID],
        Some(HOME_SESSION_COOKIE),
    )
    .map(|required| required.context)
}

pub(crate) fn home_launch_token_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-elastos-home-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn require_home_launch_token_for_any_from(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
    cookie_name: Option<&str>,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    if home_launch_token_header(headers)
        .or_else(|| cookie_name.and_then(|name| cookie_value_from_headers(headers, name)))
        .is_none()
    {
        anyhow::bail!("missing home launch token");
    }
    let expected_did = load_existing_gateway_runtime_did(data_dir)
        .ok_or_else(|| anyhow::anyhow!("gateway identity is unavailable"))?;
    let auth_data_dir = home_launch_auth_data_dir(data_dir);
    require_home_launch_token_for_any_from_expected_did(
        data_dir,
        headers,
        allowed_apps,
        cookie_name,
        expected_did,
        &auth_data_dir,
    )
}

fn require_home_launch_token_for_any_from_expected_did(
    _data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
    cookie_name: Option<&str>,
    expected_did: String,
    auth_data_dir: &std::path::Path,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    let token = home_launch_token_header(headers)
        .or_else(|| cookie_name.and_then(|name| cookie_value_from_headers(headers, name)))
        .ok_or_else(|| anyhow::anyhow!("missing home launch token"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_str())
        .map_err(|_| anyhow::anyhow!("invalid home launch token encoding"))?;
    let envelope: HomeLaunchTokenEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid home launch token payload"))?;
    if envelope.payload.schema != "elastos.home.launch-token/v2" {
        anyhow::bail!("unsupported home launch token schema");
    }
    let expected_dids = vec![expected_did];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        HOME_LAUNCH_TOKEN_DOMAIN,
        &expected_dids,
    )
    .map_err(|err| anyhow::anyhow!("invalid home launch token: {}", err))?;
    if !allowed_apps.iter().any(|app| envelope.payload.app == *app) {
        anyhow::bail!("home launch token is not authorized for this provider");
    }
    require_capsule_browser_origin(headers, &envelope.payload.app)?;
    if envelope.payload.exp <= now_ts() {
        anyhow::bail!("home launch token expired");
    }
    if !envelope.payload.non_delegatable {
        anyhow::bail!("home launch token must be non-delegatable");
    }
    if envelope.payload.session_id.trim().is_empty()
        || envelope.payload.principal_id.trim().is_empty()
        || envelope.payload.grant_id.trim().is_empty()
    {
        anyhow::bail!("home launch token is missing authority context");
    }
    if envelope.payload.proof_binding_id.is_some()
        && !crate::auth::is_auth_session_active(
            auth_data_dir,
            &envelope.payload.session_id,
            now_ts(),
        )?
    {
        anyhow::bail!("home launch token auth session is not active");
    }
    Ok(RequiredHomeLaunchToken {
        app: envelope.payload.app,
        context: HomeLaunchTokenContext {
            principal_id: envelope.payload.principal_id,
            session_id: envelope.payload.session_id,
            proof_binding_id: envelope.payload.proof_binding_id,
            grant_id: envelope.payload.grant_id,
        },
    })
}

fn require_capsule_browser_origin(headers: &HeaderMap, app: &str) -> anyhow::Result<()> {
    let browser_request = headers.contains_key(axum::http::header::ORIGIN)
        || headers.contains_key(axum::http::header::REFERER)
        || headers.contains_key("sec-fetch-site");
    if !browser_request || app == HOME_CAPSULE_ID {
        return Ok(());
    }
    headers
        .get(axum::http::header::HOST)
        .ok_or_else(|| anyhow::anyhow!("capsule browser request is missing its host"))?
        .to_str()
        .map_err(|_| anyhow::anyhow!("capsule browser request has an invalid host"))?
        .parse::<axum::http::uri::Authority>()
        .map_err(|_| anyhow::anyhow!("capsule browser request has an invalid host"))?;
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .ok_or_else(|| anyhow::anyhow!("capsule browser request is missing its opaque origin"))?
        .to_str()
        .map_err(|_| anyhow::anyhow!("capsule browser request has an invalid origin"))?;
    if origin != "null" {
        anyhow::bail!("home launch token requires an opaque capsule origin");
    }
    Ok(())
}

pub(crate) fn home_launch_auth_data_dir(data_dir: &std::path::Path) -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_HOME_LAUNCH_AUTH_DATA_DIR.with(|value| value.borrow().clone()) {
        return path;
    }
    std::env::var_os(HOME_LAUNCH_TRUSTED_AUTH_DATA_DIR_ENV)
        .and_then(|value| {
            let value = value.into_string().ok()?;
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(PathBuf::from(value))
            }
        })
        .unwrap_or_else(|| data_dir.to_path_buf())
}

#[cfg(test)]
thread_local! {
    static TEST_HOME_LAUNCH_AUTH_DATA_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) struct TestHomeLaunchAuthDataDir {
    previous: Option<PathBuf>,
}

#[cfg(test)]
impl Drop for TestHomeLaunchAuthDataDir {
    fn drop(&mut self) {
        TEST_HOME_LAUNCH_AUTH_DATA_DIR.with(|value| {
            *value.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn set_test_home_launch_auth_data_dir(
    data_dir: &std::path::Path,
) -> TestHomeLaunchAuthDataDir {
    let previous = TEST_HOME_LAUNCH_AUTH_DATA_DIR
        .with(|value| value.borrow_mut().replace(data_dir.to_path_buf()));
    TestHomeLaunchAuthDataDir { previous }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_launch_token_requires_an_opaque_capsule_origin() {
        let data_dir = tempfile::tempdir().unwrap();
        elastos_identity::load_or_create_did(data_dir.path()).unwrap();
        let token = issue_home_launch_token(data_dir.path(), "browser").unwrap();

        let mut valid = HeaderMap::new();
        valid.insert("x-elastos-home-token", token.parse().unwrap());
        valid.insert("host", "localhost:61180".parse().unwrap());
        valid.insert("origin", "null".parse().unwrap());
        require_home_launch_token(data_dir.path(), &valid, "browser").unwrap();

        let mut stolen = valid.clone();
        stolen.insert("origin", "https://evil.example".parse().unwrap());
        let error = require_home_launch_token(data_dir.path(), &stolen, "browser").unwrap_err();
        assert!(error
            .to_string()
            .contains("requires an opaque capsule origin"));

        let mut missing_origin = valid;
        missing_origin.remove("origin");
        missing_origin.insert("sec-fetch-site", "cross-site".parse().unwrap());
        let error =
            require_home_launch_token(data_dir.path(), &missing_origin, "browser").unwrap_err();
        assert!(error.to_string().contains("missing its opaque origin"));
    }

    #[test]
    fn fresh_passkey_proof_is_app_scoped_and_single_use() {
        let data_dir = tempfile::tempdir().unwrap();
        elastos_identity::load_or_create_did(data_dir.path()).unwrap();
        let now = now_ts();
        let context = HomeLaunchTokenContext {
            principal_id: "person:local:alice".to_string(),
            session_id: "auth:alice".to_string(),
            proof_binding_id: Some("proof:passkey:alice".to_string()),
            grant_id: "grant:alice".to_string(),
        };
        crate::auth::store_session_grant(
            data_dir.path(),
            elastos_runtime::auth::AuthSessionGrantV1 {
                schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
                grant_id: context.grant_id.clone(),
                session_id: context.session_id.clone(),
                principal_id: context.principal_id.clone(),
                proof_binding_id: context.proof_binding_id.clone().unwrap(),
                issued_at: now,
                expires_at: now + HOME_LAUNCH_TOKEN_TTL_SECS,
                apps: vec![INBOX_CAPSULE_ID.to_string()],
            },
        )
        .unwrap();
        let request = serde_json::json!({ "request_id": "inspect-action-1" });
        let token = issue_home_launch_token_with_intent(
            data_dir.path(),
            INBOX_CAPSULE_ID,
            &context,
            "inspect.approve",
            &request,
        )
        .unwrap();

        let wrong_app = consume_fresh_passkey_home_token(
            data_dir.path(),
            &token,
            &context,
            SYSTEM_CAPSULE_ID,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(wrong_app.to_string().contains("not authorized"));
        consume_fresh_passkey_home_token(
            data_dir.path(),
            &token,
            &context,
            INBOX_CAPSULE_ID,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap();
        let replay = consume_fresh_passkey_home_token(
            data_dir.path(),
            &token,
            &context,
            INBOX_CAPSULE_ID,
            180,
            "inspect.approve",
            &request,
        )
        .unwrap_err();
        assert!(replay.to_string().contains("already been used"));
    }

    #[test]
    fn fresh_passkey_proof_rejects_substituted_intent() {
        let data_dir = tempfile::tempdir().unwrap();
        elastos_identity::load_or_create_did(data_dir.path()).unwrap();
        let now = now_ts();
        let context = HomeLaunchTokenContext {
            principal_id: "person:local:alice".to_string(),
            session_id: "auth:alice".to_string(),
            proof_binding_id: Some("proof:passkey:alice".to_string()),
            grant_id: "grant:alice".to_string(),
        };
        crate::auth::store_session_grant(
            data_dir.path(),
            elastos_runtime::auth::AuthSessionGrantV1 {
                schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
                grant_id: context.grant_id.clone(),
                session_id: context.session_id.clone(),
                principal_id: context.principal_id.clone(),
                proof_binding_id: context.proof_binding_id.clone().unwrap(),
                issued_at: now,
                expires_at: now + HOME_LAUNCH_TOKEN_TTL_SECS,
                apps: vec![INBOX_CAPSULE_ID.to_string()],
            },
        )
        .unwrap();
        let approved = serde_json::json!({ "request_id": "inspect-action-1" });
        let token = issue_home_launch_token_with_intent(
            data_dir.path(),
            INBOX_CAPSULE_ID,
            &context,
            "inspect.approve",
            &approved,
        )
        .unwrap();

        let wrong_operation = consume_fresh_passkey_home_token(
            data_dir.path(),
            &token,
            &context,
            INBOX_CAPSULE_ID,
            180,
            "wallet.approve",
            &approved,
        )
        .unwrap_err();
        assert!(wrong_operation.to_string().contains("intent mismatch"));
        let wrong_request = consume_fresh_passkey_home_token(
            data_dir.path(),
            &token,
            &context,
            INBOX_CAPSULE_ID,
            180,
            "inspect.approve",
            &serde_json::json!({ "request_id": "inspect-action-2" }),
        )
        .unwrap_err();
        assert!(wrong_request.to_string().contains("intent mismatch"));
    }

    #[test]
    fn launch_grant_accepts_explicit_parent_gateway_signer() {
        let parent = tempfile::tempdir().unwrap();
        let child = tempfile::tempdir().unwrap();
        let (_parent_key, parent_did) =
            elastos_identity::load_or_create_did(parent.path()).unwrap();
        let (_child_key, child_did) = elastos_identity::load_or_create_did(child.path()).unwrap();
        assert_ne!(parent_did, child_did);

        let context = HomeLaunchTokenContext {
            principal_id: "person:local:alice".to_string(),
            session_id: "session:alice".to_string(),
            proof_binding_id: None,
            grant_id: "grant:alice".to_string(),
        };
        let token =
            issue_home_launch_token_with_context(parent.path(), "marketplace", &context).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-elastos-home-token", token.parse().unwrap());

        let child_result = require_home_launch_token_for_any_from_expected_did(
            child.path(),
            &headers,
            &["marketplace"],
            None,
            child_did,
            child.path(),
        );
        match child_result {
            Ok(_) => panic!("child runtime DID should not verify parent gateway grant"),
            Err(err) => assert!(err.to_string().contains("Signer DID mismatch")),
        }

        let parent_result = require_home_launch_token_for_any_from_expected_did(
            child.path(),
            &headers,
            &["marketplace"],
            None,
            parent_did,
            child.path(),
        )
        .unwrap();
        assert_eq!(parent_result.context.principal_id, "person:local:alice");
    }

    #[test]
    fn launch_grant_checks_parent_auth_sessions_for_child_runtime() {
        let parent = tempfile::tempdir().unwrap();
        let child = tempfile::tempdir().unwrap();
        let (_parent_key, parent_did) =
            elastos_identity::load_or_create_did(parent.path()).unwrap();
        let (_child_key, child_did) = elastos_identity::load_or_create_did(child.path()).unwrap();
        assert_ne!(parent_did, child_did);

        let now = now_ts();
        let context = HomeLaunchTokenContext {
            principal_id: "person:local:alice".to_string(),
            session_id: "auth:alice".to_string(),
            proof_binding_id: Some("proof:passkey:alice".to_string()),
            grant_id: "grant:alice".to_string(),
        };
        crate::auth::store_session_grant(
            parent.path(),
            elastos_runtime::auth::AuthSessionGrantV1 {
                schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
                grant_id: context.grant_id.clone(),
                session_id: context.session_id.clone(),
                principal_id: context.principal_id.clone(),
                proof_binding_id: context.proof_binding_id.clone().unwrap(),
                issued_at: now,
                expires_at: now + HOME_LAUNCH_TOKEN_TTL_SECS,
                apps: vec!["home".to_string(), "marketplace".to_string()],
            },
        )
        .unwrap();

        let token =
            issue_home_launch_token_with_context(parent.path(), "marketplace", &context).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-elastos-home-token", token.parse().unwrap());

        let child_result = require_home_launch_token_for_any_from_expected_did(
            child.path(),
            &headers,
            &["marketplace"],
            None,
            parent_did.clone(),
            child.path(),
        );
        match child_result {
            Ok(_) => panic!("child runtime must not use its own auth state for parent grants"),
            Err(err) => assert!(err
                .to_string()
                .contains("home launch token auth session is not active")),
        }

        let parent_auth_result = require_home_launch_token_for_any_from_expected_did(
            child.path(),
            &headers,
            &["marketplace"],
            None,
            parent_did,
            parent.path(),
        )
        .unwrap();
        assert_eq!(parent_auth_result.context.session_id, "auth:alice");
    }
}
