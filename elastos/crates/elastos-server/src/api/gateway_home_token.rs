use axum::http::{HeaderMap, HeaderValue};
use base64::Engine as _;
#[cfg(test)]
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
        "{}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax",
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
        format!("{name}={token}; Max-Age={max_age_secs}; Path={path}; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn uuid_like_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
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

pub(crate) fn require_fresh_passkey_home_token(
    data_dir: &std::path::Path,
    token: &str,
    expected_context: &HomeLaunchTokenContext,
    max_age_secs: u64,
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
    if envelope.payload.app != HOME_CAPSULE_ID {
        anyhow::bail!("fresh passkey token must be a Home token");
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
    {
        anyhow::bail!("fresh passkey token authority context mismatch");
    }
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
    let local_did = load_existing_gateway_runtime_did(data_dir)
        .ok_or_else(|| anyhow::anyhow!("gateway identity is unavailable"))?;
    let expected_dids = vec![local_did];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        HOME_LAUNCH_TOKEN_DOMAIN,
        &expected_dids,
    )
    .map_err(|err| anyhow::anyhow!("invalid home launch token: {}", err))?;
    if !allowed_apps.iter().any(|app| envelope.payload.app == *app) {
        anyhow::bail!("home launch token is not authorized for this provider");
    }
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
        && !crate::auth::is_auth_session_active(data_dir, &envelope.payload.session_id, now_ts())?
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
