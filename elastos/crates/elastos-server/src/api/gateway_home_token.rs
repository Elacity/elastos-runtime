use axum::http::{HeaderMap, HeaderValue};
use base64::Engine as _;
use elastos_wallet_contract::VerifiedWalletInvocationContext;
use rand::RngCore;

use super::*;

const HOME_LAUNCH_TOKEN_SCHEMA: &str = "elastos.home.launch-token/v4";
const HOME_LAUNCH_CONTEXT_SCHEMA: &str = "elastos.runtime.browser-launch/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HomeLaunchTokenContext {
    pub principal_id: String,
    pub session_id: String,
    pub proof_binding_id: Option<String>,
    pub grant_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(in crate::api) struct HomeLaunchContext {
    pub(in crate::api) schema: String,
    pub(in crate::api) selected_resource: String,
    pub(in crate::api) executable_actor: String,
    pub(in crate::api) authority_actor: String,
}

impl HomeLaunchContext {
    fn direct(actor: &str) -> Self {
        Self {
            schema: HOME_LAUNCH_CONTEXT_SCHEMA.to_string(),
            selected_resource: actor.to_string(),
            executable_actor: actor.to_string(),
            authority_actor: actor.to_string(),
        }
    }

    fn projection(selected_resource: &str, executable_actor: &str) -> Self {
        Self {
            schema: HOME_LAUNCH_CONTEXT_SCHEMA.to_string(),
            selected_resource: selected_resource.to_string(),
            executable_actor: executable_actor.to_string(),
            authority_actor: HOME_CAPSULE_ID.to_string(),
        }
    }

    pub(in crate::api) fn validate(&self) -> anyhow::Result<()> {
        if self.schema != HOME_LAUNCH_CONTEXT_SCHEMA
            || self.selected_resource.trim().is_empty()
            || self.executable_actor.trim().is_empty()
            || self.authority_actor.trim().is_empty()
            || self.selected_resource.len() > 256
            || self.executable_actor.len() > 256
            || self.authority_actor.len() > 256
            || (self.authority_actor != self.executable_actor
                && self.authority_actor != HOME_CAPSULE_ID)
        {
            anyhow::bail!("home launch token has an invalid Runtime launch context");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeLaunchTokenPayload {
    schema: String,
    launch_id: String,
    launch_context: HomeLaunchContext,
    iat: u64,
    exp: u64,
    principal_id: String,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_binding_id: Option<String>,
    grant_id: String,
    non_delegatable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeLaunchTokenEnvelope {
    payload: HomeLaunchTokenPayload,
    signature: String,
    signer_did: String,
}

#[derive(Debug, Clone)]
pub(in crate::api) struct RequiredHomeLaunchToken {
    pub(in crate::api) launch_id: String,
    pub(in crate::api) launch_context: HomeLaunchContext,
    pub(in crate::api) context: HomeLaunchTokenContext,
}

/// Runtime-owned Wallet authority produced only after launch-token validation.
#[derive(Debug, Clone)]
pub(in crate::api) struct RuntimeWalletAuthority {
    context: VerifiedWalletInvocationContext,
}

impl RuntimeWalletAuthority {
    pub(in crate::api) fn verified_context(&self) -> &VerifiedWalletInvocationContext {
        &self.context
    }

    pub(in crate::api) fn home_launch_context(&self) -> HomeLaunchTokenContext {
        HomeLaunchTokenContext {
            principal_id: self.context.principal_id().to_string(),
            session_id: self.context.session_id().to_string(),
            proof_binding_id: self.context.proof_binding_id().map(ToString::to_string),
            grant_id: self.context.grant_id().to_string(),
        }
    }

    #[cfg(test)]
    pub(super) fn from_verified_context(context: VerifiedWalletInvocationContext) -> Self {
        Self { context }
    }
}

#[derive(Clone, Copy)]
enum HomeLaunchOriginPolicy {
    Browser,
    InternalShell,
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
    issue_home_launch_token_at(data_dir, &HomeLaunchContext::direct(app), context, now_ts())
}

pub(crate) fn issue_home_projection_launch_token_with_context(
    data_dir: &std::path::Path,
    selected_resource: &str,
    executable_actor: &str,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<String> {
    issue_home_launch_token_at(
        data_dir,
        &HomeLaunchContext::projection(selected_resource, executable_actor),
        context,
        now_ts(),
    )
}

fn issue_home_launch_token_at(
    data_dir: &std::path::Path,
    launch_context: &HomeLaunchContext,
    context: &HomeLaunchTokenContext,
    issued_at: u64,
) -> anyhow::Result<String> {
    launch_context.validate()?;
    if context.principal_id.trim().is_empty()
        || context.session_id.trim().is_empty()
        || context.grant_id.trim().is_empty()
    {
        anyhow::bail!("home launch token is missing authority context");
    }
    let (signing_key, _did) = elastos_identity::load_or_create_did(data_dir)?;
    let envelope = HomeLaunchTokenEnvelope {
        payload: HomeLaunchTokenPayload {
            schema: HOME_LAUNCH_TOKEN_SCHEMA.to_string(),
            launch_id: format!("launch:{}", uuid_like_token()),
            launch_context: launch_context.clone(),
            iat: issued_at,
            exp: issued_at.saturating_add(HOME_LAUNCH_TOKEN_TTL_SECS),
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

#[cfg(test)]
pub(crate) fn issue_expired_home_launch_token_with_context(
    data_dir: &std::path::Path,
    app: &str,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<String> {
    issue_home_launch_token_at(
        data_dir,
        &HomeLaunchContext::direct(app),
        context,
        now_ts().saturating_sub(HOME_LAUNCH_TOKEN_TTL_SECS + 1),
    )
}

pub(crate) fn require_home_launch_token(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    expected_app: &str,
) -> anyhow::Result<()> {
    require_home_launch_token_context(data_dir, headers, expected_app).map(|_| ())
}

pub(crate) fn require_home_launch_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    expected_app: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_from_with_origin(
        data_dir,
        headers,
        &[expected_app],
        None,
        HomeLaunchOriginPolicy::Browser,
    )
    .map(|required| required.context)
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
        .map(|required| (required.launch_context.executable_actor, required.context))
}

pub(crate) fn require_home_projection_launch_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    selected_resource: &str,
    executable_actor: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    let required =
        require_home_launch_token_for_any_from(data_dir, headers, &[executable_actor], None)?;
    if required.launch_context.selected_resource != selected_resource
        || required.launch_context.executable_actor != executable_actor
    {
        anyhow::bail!("home launch token projection authority mismatch");
    }
    Ok(required.context)
}

pub(crate) fn require_home_viewer_launch_token_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    executable_actor: &str,
) -> anyhow::Result<(String, HomeLaunchTokenContext)> {
    let required =
        require_home_launch_token_for_any_from(data_dir, headers, &[executable_actor], None)?;
    Ok((required.launch_context.selected_resource, required.context))
}

pub(crate) fn require_internal_shell_launch_grant_for_any_context(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_for_any_from_with_origin(
        data_dir,
        headers,
        allowed_apps,
        None,
        HomeLaunchOriginPolicy::InternalShell,
    )
    .map(|required| required.context)
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
    require_home_launch_token_for_any_from_with_origin(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID],
        Some(HOME_SESSION_COOKIE),
        HomeLaunchOriginPolicy::Browser,
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

/// Project a verified launch-token v4 context into private Wallet Bus authority.
pub(in crate::api) fn require_runtime_wallet_authority(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<RuntimeWalletAuthority> {
    let required = require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)?;
    runtime_wallet_authority(&required)
}

pub(in crate::api) fn require_home_runtime_wallet_authority(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
) -> anyhow::Result<RuntimeWalletAuthority> {
    let required = require_home_launch_token_for_any_from_with_origin(
        data_dir,
        headers,
        &[HOME_CAPSULE_ID],
        Some(HOME_SESSION_COOKIE),
        HomeLaunchOriginPolicy::Browser,
    )?;
    runtime_wallet_authority(&required)
}

pub(in crate::api) fn runtime_wallet_authority(
    required: &RequiredHomeLaunchToken,
) -> anyhow::Result<RuntimeWalletAuthority> {
    let context = VerifiedWalletInvocationContext::new(
        required.context.principal_id.clone(),
        required.context.session_id.clone(),
        required.context.proof_binding_id.clone(),
        required.context.grant_id.clone(),
        required.launch_context.executable_actor.clone(),
        required.launch_id.clone(),
    )
    .map_err(|err| anyhow::anyhow!("invalid verified Wallet authority: {err}"))?;
    Ok(RuntimeWalletAuthority { context })
}

pub(in crate::api) fn require_home_launch_token_binding(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
) -> anyhow::Result<RequiredHomeLaunchToken> {
    require_home_launch_token_for_any_from(data_dir, headers, allowed_apps, None)
}

fn require_home_launch_token_for_any_from(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
    cookie_name: Option<&str>,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    require_home_launch_token_for_any_from_with_origin(
        data_dir,
        headers,
        allowed_apps,
        cookie_name,
        HomeLaunchOriginPolicy::Browser,
    )
}

fn require_home_launch_token_for_any_from_with_origin(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
    cookie_name: Option<&str>,
    origin_policy: HomeLaunchOriginPolicy,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    let (header_token, cookie_token) = home_launch_token_candidates(headers, cookie_name)?;
    if header_token.is_none() && cookie_token.is_none() {
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
        origin_policy,
    )
}

fn require_home_launch_token_for_any_from_expected_did(
    _data_dir: &std::path::Path,
    headers: &HeaderMap,
    allowed_apps: &[&str],
    cookie_name: Option<&str>,
    expected_did: String,
    auth_data_dir: &std::path::Path,
    origin_policy: HomeLaunchOriginPolicy,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    let (header_token, cookie_token) = home_launch_token_candidates(headers, cookie_name)?;
    let required = match (header_token.as_deref(), cookie_token.as_deref()) {
        (None, None) => anyhow::bail!("missing home launch token"),
        (Some(token), None) | (None, Some(token)) => {
            require_home_launch_token_value_from_expected_did(
                token,
                allowed_apps,
                expected_did,
                auth_data_dir,
            )?
        }
        (Some(header), Some(cookie)) if header == cookie => {
            require_home_launch_token_value_from_expected_did(
                header,
                allowed_apps,
                expected_did,
                auth_data_dir,
            )?
        }
        (Some(header), Some(cookie)) => {
            // Session refresh rotates the cookie while open tabs still hold
            // the prior mint, so a divergent pair is only a conflict when the
            // two tokens verify to different session authorities. Same
            // authority answers with the cookie, the newest server-set mint.
            let from_cookie = require_home_launch_token_value_from_expected_did(
                cookie,
                allowed_apps,
                expected_did.clone(),
                auth_data_dir,
            );
            let from_header = require_home_launch_token_value_from_expected_did(
                header,
                allowed_apps,
                expected_did,
                auth_data_dir,
            );
            match (from_cookie, from_header) {
                (Ok(from_cookie), Ok(from_header))
                    if from_cookie.context == from_header.context =>
                {
                    from_cookie
                }
                _ => anyhow::bail!("conflicting Home launch-token authorities"),
            }
        }
    };
    match origin_policy {
        HomeLaunchOriginPolicy::Browser
            if required.launch_context.executable_actor == HOME_CAPSULE_ID =>
        {
            require_exact_home_browser_origin(headers)?
        }
        HomeLaunchOriginPolicy::Browser => require_capsule_browser_origin(headers)?,
        HomeLaunchOriginPolicy::InternalShell => require_internal_shell_origin(headers)?,
    }
    Ok(required)
}

pub(in crate::api) fn require_carried_home_launch_token(
    data_dir: &std::path::Path,
    token: &str,
    allowed_apps: &[&str],
) -> anyhow::Result<RequiredHomeLaunchToken> {
    let expected_did = load_existing_gateway_runtime_did(data_dir)
        .ok_or_else(|| anyhow::anyhow!("gateway identity is unavailable"))?;
    let auth_data_dir = home_launch_auth_data_dir(data_dir);
    require_home_launch_token_value_from_expected_did(
        token,
        allowed_apps,
        expected_did,
        &auth_data_dir,
    )
}

fn require_home_launch_token_value_from_expected_did(
    token: &str,
    allowed_apps: &[&str],
    expected_did: String,
    auth_data_dir: &std::path::Path,
) -> anyhow::Result<RequiredHomeLaunchToken> {
    let token = token.trim();
    if token.is_empty() || token.len() > 16 * 1024 {
        anyhow::bail!("invalid home launch token encoding");
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| anyhow::anyhow!("invalid home launch token encoding"))?;
    require_launch_token_schema(&bytes)?;
    let envelope: HomeLaunchTokenEnvelope = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid home launch token payload"))?;
    let expected_dids = vec![expected_did];
    crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        HOME_LAUNCH_TOKEN_DOMAIN,
        &expected_dids,
    )
    .map_err(|err| anyhow::anyhow!("invalid home launch token: {}", err))?;
    envelope.payload.launch_context.validate()?;
    if !valid_home_launch_id(&envelope.payload.launch_id) {
        anyhow::bail!("home launch token has an invalid launch id");
    }
    if !allowed_apps
        .iter()
        .any(|app| envelope.payload.launch_context.executable_actor == *app)
    {
        anyhow::bail!("home launch token is not authorized for this provider");
    }
    let now = now_ts();
    if envelope.payload.exp <= now {
        anyhow::bail!("home launch token expired");
    }
    if envelope.payload.iat > now.saturating_add(60)
        || envelope.payload.exp <= envelope.payload.iat
        || envelope.payload.exp.saturating_sub(envelope.payload.iat) > HOME_LAUNCH_TOKEN_TTL_SECS
    {
        anyhow::bail!("home launch token lifetime is invalid");
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
    match envelope.payload.proof_binding_id.as_deref() {
        Some(proof_binding_id) if proof_binding_id.trim().is_empty() => {
            anyhow::bail!("home launch token is missing authority context")
        }
        Some(proof_binding_id) => {
            let grant = crate::auth::load_active_session_grant(
                auth_data_dir,
                &envelope.payload.session_id,
                now,
            )
            .map_err(|_| anyhow::anyhow!("home launch token auth session is not active"))?;
            if grant.principal_id != envelope.payload.principal_id
                || grant.proof_binding_id != proof_binding_id
                || grant.grant_id != envelope.payload.grant_id
                || !grant
                    .apps
                    .iter()
                    .any(|app| app == &envelope.payload.launch_context.authority_actor)
            {
                anyhow::bail!("home launch token authority context mismatch");
            }
        }
        None => {
            let expected_principal =
                elastos_runtime::auth::PrincipalId::device_did(&expected_dids[0]);
            if envelope.payload.principal_id != expected_principal.as_str()
                || !valid_local_authority_id(&envelope.payload.session_id, "local:")
                || !valid_local_authority_id(&envelope.payload.grant_id, "grant:local:")
            {
                anyhow::bail!(
                    "proofless home launch token is not exact Runtime-local device authority"
                );
            }
        }
    }
    Ok(RequiredHomeLaunchToken {
        launch_id: envelope.payload.launch_id,
        launch_context: envelope.payload.launch_context,
        context: HomeLaunchTokenContext {
            principal_id: envelope.payload.principal_id,
            session_id: envelope.payload.session_id,
            proof_binding_id: envelope.payload.proof_binding_id,
            grant_id: envelope.payload.grant_id,
        },
    })
}

fn require_launch_token_schema(bytes: &[u8]) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid home launch token payload"))?;
    if value
        .get("payload")
        .and_then(|payload| payload.get("schema"))
        .and_then(serde_json::Value::as_str)
        != Some(HOME_LAUNCH_TOKEN_SCHEMA)
    {
        anyhow::bail!("unsupported home launch token schema");
    }
    Ok(())
}

fn valid_home_launch_id(value: &str) -> bool {
    value
        .strip_prefix("launch:")
        .is_some_and(|suffix| suffix.len() == 32 && suffix.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn valid_local_authority_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|suffix| suffix.len() == 32 && suffix.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn require_capsule_browser_origin(headers: &HeaderMap) -> anyhow::Result<()> {
    single_browser_header(headers, axum::http::header::HOST.as_str())?
        .ok_or_else(|| anyhow::anyhow!("capsule browser request is missing its host"))?
        .parse::<axum::http::uri::Authority>()
        .map_err(|_| anyhow::anyhow!("capsule browser request has an invalid host"))?;
    let origin = single_browser_header(headers, axum::http::header::ORIGIN.as_str())?
        .ok_or_else(|| anyhow::anyhow!("capsule browser request is missing its opaque origin"))?;
    if origin != "null" {
        anyhow::bail!("home launch token requires an opaque capsule origin");
    }
    Ok(())
}

fn require_exact_home_browser_origin(headers: &HeaderMap) -> anyhow::Result<()> {
    let host = single_browser_header(headers, axum::http::header::HOST.as_str())?
        .ok_or_else(|| anyhow::anyhow!("Home browser request is missing its host"))?;
    let authority = host
        .parse::<axum::http::uri::Authority>()
        .map_err(|_| anyhow::anyhow!("Home browser request has an invalid host"))?;
    let scheme = if super::request_uses_tls(headers) {
        "https"
    } else {
        "http"
    };
    let expected = url::Url::parse(&format!("{scheme}://{authority}"))?
        .origin()
        .ascii_serialization();
    let mut has_same_origin_provenance = false;
    if let Some(origin) = single_browser_header(headers, axum::http::header::ORIGIN.as_str())? {
        let parsed = url::Url::parse(origin)
            .map_err(|_| anyhow::anyhow!("Home browser request has an invalid origin"))?;
        if origin != expected || parsed.origin().ascii_serialization() != expected {
            anyhow::bail!("Home browser request requires the exact destination origin");
        }
        has_same_origin_provenance = true;
    }
    if let Some(referer) = single_browser_header(headers, axum::http::header::REFERER.as_str())? {
        let parsed = url::Url::parse(referer)
            .map_err(|_| anyhow::anyhow!("Home browser request has an invalid referer"))?;
        if parsed.origin().ascii_serialization() != expected
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            anyhow::bail!("Home browser request requires the exact destination origin");
        }
        has_same_origin_provenance = true;
    }
    if let Some(site) = single_browser_header(headers, "sec-fetch-site")? {
        if site != "same-origin" {
            anyhow::bail!("Home browser request requires a same-origin fetch context");
        }
        has_same_origin_provenance = true;
    }
    if !has_same_origin_provenance {
        anyhow::bail!("Home browser request is missing same-origin provenance");
    }
    Ok(())
}

fn require_internal_shell_origin(headers: &HeaderMap) -> anyhow::Result<()> {
    if headers.contains_key(axum::http::header::ORIGIN)
        || headers.contains_key(axum::http::header::REFERER)
        || headers.contains_key("sec-fetch-site")
    {
        anyhow::bail!("internal launch transfer must not carry browser provenance");
    }
    Ok(())
}

fn single_home_launch_token_header(headers: &HeaderMap) -> anyhow::Result<Option<String>> {
    let mut values = headers.get_all("x-elastos-home-token").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        anyhow::bail!("duplicate home launch token header");
    }
    let value = value
        .to_str()
        .map_err(|_| anyhow::anyhow!("invalid home launch token header"))?
        .trim();
    if value.is_empty() {
        return Ok(None);
    }
    Ok(Some(value.to_string()))
}

fn home_launch_token_candidates(
    headers: &HeaderMap,
    cookie_name: Option<&str>,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let header = single_home_launch_token_header(headers)?;
    let cookie = match cookie_name {
        Some(name) => single_cookie_value(headers, name)?,
        None => None,
    };
    Ok((header, cookie))
}

fn single_cookie_value(headers: &HeaderMap, name: &str) -> anyhow::Result<Option<String>> {
    let mut cookie_headers = headers.get_all(axum::http::header::COOKIE).iter();
    let Some(cookie_header) = cookie_headers.next() else {
        return Ok(None);
    };
    if cookie_headers.next().is_some() {
        anyhow::bail!("duplicate Cookie headers");
    }
    let cookie_header = cookie_header
        .to_str()
        .map_err(|_| anyhow::anyhow!("invalid Cookie header"))?;
    let mut values = cookie_header.split(';').filter_map(|entry| {
        let (key, value) = entry.trim().split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_string())
    });
    let value = values.next().filter(|value| !value.is_empty());
    if values.next().is_some() {
        anyhow::bail!("duplicate {name} cookies");
    }
    Ok(value)
}

fn single_browser_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> anyhow::Result<Option<&'a str>> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        anyhow::bail!("browser request has duplicate {name} headers");
    }
    value
        .to_str()
        .map(Some)
        .map_err(|_| anyhow::anyhow!("browser request has an invalid {name} header"))
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

    fn capsule_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-elastos-home-token", token.parse().unwrap());
        headers.insert("host", "localhost:61180".parse().unwrap());
        headers.insert("origin", "null".parse().unwrap());
        headers
    }

    fn decode_launch_token(token: &str) -> HomeLaunchTokenEnvelope {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn sign_launch_token_for_test(
        data_dir: &std::path::Path,
        mut envelope: HomeLaunchTokenEnvelope,
        domain: &str,
    ) -> String {
        let (signing_key, signer_did) = elastos_identity::load_or_create_did(data_dir).unwrap();
        let canonical =
            serde_json::to_string(&serde_json::to_value(&envelope.payload).unwrap()).unwrap();
        let (signature, _) =
            crate::crypto::domain_separated_sign(&signing_key, domain, canonical.as_bytes());
        envelope.signature = signature;
        envelope.signer_did = signer_did;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&envelope).unwrap())
    }

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

        let mut missing_host = capsule_headers(&token);
        missing_host.remove("host");
        assert!(
            require_home_launch_token(data_dir.path(), &missing_host, "browser")
                .unwrap_err()
                .to_string()
                .contains("missing its host")
        );

        let mut duplicate_origin = capsule_headers(&token);
        duplicate_origin.append("origin", "null".parse().unwrap());
        assert!(
            require_home_launch_token(data_dir.path(), &duplicate_origin, "browser")
                .unwrap_err()
                .to_string()
                .contains("duplicate origin")
        );
    }

    #[test]
    fn projection_token_binds_resource_actor_and_unique_launch() {
        let data_dir = tempfile::tempdir().unwrap();
        let context = local_home_launch_token_context(data_dir.path()).unwrap();
        let first = issue_home_projection_launch_token_with_context(
            data_dir.path(),
            "gba-ucity",
            "gba-emulator",
            &context,
        )
        .unwrap();
        let second = issue_home_projection_launch_token_with_context(
            data_dir.path(),
            "gba-ucity",
            "gba-emulator",
            &context,
        )
        .unwrap();
        let first_envelope = decode_launch_token(&first);
        let second_envelope = decode_launch_token(&second);
        assert_eq!(
            first_envelope.payload.launch_context.authority_actor,
            HOME_CAPSULE_ID
        );
        assert_eq!(
            first_envelope.payload.launch_context.executable_actor,
            "gba-emulator"
        );
        assert_eq!(first_envelope.payload.iat, second_envelope.payload.iat);
        assert_ne!(
            first_envelope.payload.launch_id,
            second_envelope.payload.launch_id
        );

        let headers = capsule_headers(&first);
        require_home_projection_launch_token_context(
            data_dir.path(),
            &headers,
            "gba-ucity",
            "gba-emulator",
        )
        .unwrap();
        assert!(require_home_projection_launch_token_context(
            data_dir.path(),
            &headers,
            "gba-attacker",
            "gba-emulator",
        )
        .unwrap_err()
        .to_string()
        .contains("projection authority mismatch"));
        assert!(require_home_projection_launch_token_context(
            data_dir.path(),
            &headers,
            "gba-ucity",
            "archive-manager",
        )
        .unwrap_err()
        .to_string()
        .contains("not authorized"));
    }

    #[test]
    fn proof_bound_home_projection_uses_home_as_its_non_transitive_authority() {
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
                apps: vec![HOME_CAPSULE_ID.to_string()],
            },
        )
        .unwrap();

        let token = issue_home_projection_launch_token_with_context(
            data_dir.path(),
            "chat-room",
            "chat-room",
            &context,
        )
        .unwrap();
        let envelope = decode_launch_token(&token);
        assert_eq!(
            envelope.payload.launch_context.authority_actor,
            HOME_CAPSULE_ID
        );
        assert_eq!(
            envelope.payload.launch_context.executable_actor,
            "chat-room"
        );
        require_home_projection_launch_token_context(
            data_dir.path(),
            &capsule_headers(&token),
            "chat-room",
            "chat-room",
        )
        .unwrap();
        assert!(require_home_launch_token_context(
            data_dir.path(),
            &capsule_headers(&token),
            HOME_CAPSULE_ID,
        )
        .unwrap_err()
        .to_string()
        .contains("not authorized"));

        let system_context = HomeLaunchTokenContext {
            session_id: "auth:system".to_string(),
            proof_binding_id: Some("proof:passkey:system".to_string()),
            grant_id: "grant:system".to_string(),
            ..context
        };
        crate::auth::store_session_grant(
            data_dir.path(),
            elastos_runtime::auth::AuthSessionGrantV1 {
                schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
                grant_id: system_context.grant_id.clone(),
                session_id: system_context.session_id.clone(),
                principal_id: system_context.principal_id.clone(),
                proof_binding_id: system_context.proof_binding_id.clone().unwrap(),
                issued_at: now,
                expires_at: now + HOME_LAUNCH_TOKEN_TTL_SECS,
                apps: vec![SYSTEM_CAPSULE_ID.to_string()],
            },
        )
        .unwrap();
        let unauthorized = issue_home_projection_launch_token_with_context(
            data_dir.path(),
            "chat-room",
            "chat-room",
            &system_context,
        )
        .unwrap();
        assert!(require_home_projection_launch_token_context(
            data_dir.path(),
            &capsule_headers(&unauthorized),
            "chat-room",
            "chat-room",
        )
        .unwrap_err()
        .to_string()
        .contains("authority context mismatch"));
    }

    #[test]
    fn launch_token_rejects_expiry_stale_schema_and_mixed_shape() {
        let data_dir = tempfile::tempdir().unwrap();
        let context = local_home_launch_token_context(data_dir.path()).unwrap();
        let expired =
            issue_expired_home_launch_token_with_context(data_dir.path(), "browser", &context)
                .unwrap();
        assert!(
            require_home_launch_token(data_dir.path(), &capsule_headers(&expired), "browser")
                .unwrap_err()
                .to_string()
                .contains("expired")
        );

        let valid =
            issue_home_launch_token_with_context(data_dir.path(), "browser", &context).unwrap();
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&valid)
            .unwrap();
        for stale_schema in [
            "elastos.home.launch-token/v2",
            "elastos.home.launch-token/v3",
        ] {
            let mut stale: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            stale["payload"]["schema"] = serde_json::json!(stale_schema);
            let stale = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&stale).unwrap());
            assert!(require_home_launch_token(
                data_dir.path(),
                &capsule_headers(&stale),
                "browser"
            )
            .unwrap_err()
            .to_string()
            .contains("unsupported home launch token schema"));
        }

        let valid_envelope = decode_launch_token(&valid);
        for stale_domain in ["elastos.home.launch.v1", "elastos.home.launch.v3"] {
            let stale =
                sign_launch_token_for_test(data_dir.path(), valid_envelope.clone(), stale_domain);
            assert!(require_home_launch_token(
                data_dir.path(),
                &capsule_headers(&stale),
                "browser"
            )
            .unwrap_err()
            .to_string()
            .contains("invalid home launch token"));
        }

        let mut mixed: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        mixed["payload"]["app"] = serde_json::json!("browser");
        let mixed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&mixed).unwrap());
        assert!(
            require_home_launch_token(data_dir.path(), &capsule_headers(&mixed), "browser")
                .unwrap_err()
                .to_string()
                .contains("invalid home launch token payload")
        );

        let mut missing_authority: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        missing_authority["payload"]["launch_context"]
            .as_object_mut()
            .unwrap()
            .remove("authority_actor");
        let missing_authority = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&missing_authority).unwrap());
        assert!(require_home_launch_token(
            data_dir.path(),
            &capsule_headers(&missing_authority),
            "browser"
        )
        .unwrap_err()
        .to_string()
        .contains("invalid home launch token payload"));

        let mut future = valid_envelope.clone();
        future.payload.iat = now_ts().saturating_add(61);
        future.payload.exp = future
            .payload
            .iat
            .saturating_add(HOME_LAUNCH_TOKEN_TTL_SECS);
        let future = sign_launch_token_for_test(data_dir.path(), future, HOME_LAUNCH_TOKEN_DOMAIN);
        assert!(
            require_home_launch_token(data_dir.path(), &capsule_headers(&future), "browser")
                .unwrap_err()
                .to_string()
                .contains("lifetime is invalid")
        );

        let mut overlong = valid_envelope.clone();
        overlong.payload.exp = overlong
            .payload
            .iat
            .saturating_add(HOME_LAUNCH_TOKEN_TTL_SECS + 1);
        let overlong =
            sign_launch_token_for_test(data_dir.path(), overlong, HOME_LAUNCH_TOKEN_DOMAIN);
        assert!(
            require_home_launch_token(data_dir.path(), &capsule_headers(&overlong), "browser")
                .unwrap_err()
                .to_string()
                .contains("lifetime is invalid")
        );

        let mut delegatable = valid_envelope;
        delegatable.payload.non_delegatable = false;
        let delegatable =
            sign_launch_token_for_test(data_dir.path(), delegatable, HOME_LAUNCH_TOKEN_DOMAIN);
        assert!(require_home_launch_token(
            data_dir.path(),
            &capsule_headers(&delegatable),
            "browser"
        )
        .unwrap_err()
        .to_string()
        .contains("must be non-delegatable"));
    }

    #[test]
    fn launch_token_requires_exact_session_authority_and_origin() {
        let data_dir = tempfile::tempdir().unwrap();
        elastos_identity::load_or_create_did(data_dir.path()).unwrap();
        let now = now_ts();
        let stored = HomeLaunchTokenContext {
            principal_id: "person:local:alice".to_string(),
            session_id: "auth:alice".to_string(),
            proof_binding_id: Some("proof:passkey:alice".to_string()),
            grant_id: "grant:alice".to_string(),
        };
        crate::auth::store_session_grant(
            data_dir.path(),
            elastos_runtime::auth::AuthSessionGrantV1 {
                schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
                grant_id: stored.grant_id.clone(),
                session_id: stored.session_id.clone(),
                principal_id: stored.principal_id.clone(),
                proof_binding_id: stored.proof_binding_id.clone().unwrap(),
                issued_at: now,
                expires_at: now + HOME_LAUNCH_TOKEN_TTL_SECS,
                apps: vec![SYSTEM_CAPSULE_ID.to_string()],
            },
        )
        .unwrap();
        let substituted = HomeLaunchTokenContext {
            principal_id: "person:local:bob".to_string(),
            ..stored.clone()
        };
        let token =
            issue_home_launch_token_with_context(data_dir.path(), SYSTEM_CAPSULE_ID, &substituted)
                .unwrap();
        assert!(require_home_launch_token_context(
            data_dir.path(),
            &capsule_headers(&token),
            SYSTEM_CAPSULE_ID,
        )
        .unwrap_err()
        .to_string()
        .contains("authority context mismatch"));

        for substituted in [
            HomeLaunchTokenContext {
                session_id: "auth:other".to_string(),
                ..stored.clone()
            },
            HomeLaunchTokenContext {
                proof_binding_id: Some("proof:passkey:other".to_string()),
                ..stored.clone()
            },
            HomeLaunchTokenContext {
                grant_id: "grant:other".to_string(),
                ..stored.clone()
            },
        ] {
            let token = issue_home_launch_token_with_context(
                data_dir.path(),
                SYSTEM_CAPSULE_ID,
                &substituted,
            )
            .unwrap();
            let error = require_home_launch_token_context(
                data_dir.path(),
                &capsule_headers(&token),
                SYSTEM_CAPSULE_ID,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("authority context mismatch") || error.contains("not active"));
        }

        let wrong_actor =
            issue_home_launch_token_with_context(data_dir.path(), INBOX_CAPSULE_ID, &stored)
                .unwrap();
        assert!(require_home_launch_token_context(
            data_dir.path(),
            &capsule_headers(&wrong_actor),
            INBOX_CAPSULE_ID,
        )
        .unwrap_err()
        .to_string()
        .contains("authority context mismatch"));

        let proofless = HomeLaunchTokenContext {
            proof_binding_id: None,
            ..stored.clone()
        };
        let token =
            issue_home_launch_token_with_context(data_dir.path(), SYSTEM_CAPSULE_ID, &proofless)
                .unwrap();
        assert!(require_home_launch_token_context(
            data_dir.path(),
            &capsule_headers(&token),
            SYSTEM_CAPSULE_ID,
        )
        .unwrap_err()
        .to_string()
        .contains("proofless home launch token"));

        let home_context = local_home_launch_token_context(data_dir.path()).unwrap();
        let home_token =
            issue_home_launch_token_with_context(data_dir.path(), HOME_CAPSULE_ID, &home_context)
                .unwrap();
        let mut home = HeaderMap::new();
        home.insert("x-elastos-home-token", home_token.parse().unwrap());
        home.insert("host", "localhost:45542".parse().unwrap());
        home.insert("origin", "http://localhost:45542".parse().unwrap());
        home.insert("sec-fetch-site", "same-origin".parse().unwrap());
        require_home_token_context(data_dir.path(), &home).unwrap();

        let mut same_cookie = home.clone();
        same_cookie.insert(
            axum::http::header::COOKIE,
            format!("{HOME_SESSION_COOKIE}={home_token}")
                .parse()
                .unwrap(),
        );
        require_home_token_context(data_dir.path(), &same_cookie).unwrap();

        let other_home_token = issue_home_launch_token_with_context(
            data_dir.path(),
            HOME_CAPSULE_ID,
            &local_home_launch_token_context(data_dir.path()).unwrap(),
        )
        .unwrap();
        let mut conflicting_cookie = home.clone();
        conflicting_cookie.insert(
            axum::http::header::COOKIE,
            format!("{HOME_SESSION_COOKIE}={other_home_token}")
                .parse()
                .unwrap(),
        );
        assert!(
            require_home_token_context(data_dir.path(), &conflicting_cookie)
                .unwrap_err()
                .to_string()
                .contains("conflicting Home launch-token authorities")
        );

        let rotated_token =
            issue_home_launch_token_with_context(data_dir.path(), HOME_CAPSULE_ID, &home_context)
                .unwrap();
        assert_ne!(rotated_token, home_token);
        let mut rotated_cookie = home.clone();
        rotated_cookie.insert(
            axum::http::header::COOKIE,
            format!("{HOME_SESSION_COOKIE}={rotated_token}")
                .parse()
                .unwrap(),
        );
        require_home_token_context(data_dir.path(), &rotated_cookie)
            .expect("a rotated same-session cookie beside a stale header must stay signed");

        let mut sibling = home.clone();
        sibling.insert("origin", "http://localhost:45543".parse().unwrap());
        assert!(require_home_token_context(data_dir.path(), &sibling)
            .unwrap_err()
            .to_string()
            .contains("exact destination origin"));
        let mut fetch_metadata = home.clone();
        fetch_metadata.remove("origin");
        require_home_token_context(data_dir.path(), &fetch_metadata).unwrap();

        let mut referer = home.clone();
        referer.remove("origin");
        referer.remove("sec-fetch-site");
        referer.insert(
            axum::http::header::REFERER,
            "http://localhost:45542/apps/home/".parse().unwrap(),
        );
        require_home_token_context(data_dir.path(), &referer).unwrap();

        let mut sibling_referer = referer;
        sibling_referer.insert(
            axum::http::header::REFERER,
            "http://localhost:45543/apps/home/".parse().unwrap(),
        );
        assert!(
            require_home_token_context(data_dir.path(), &sibling_referer)
                .unwrap_err()
                .to_string()
                .contains("exact destination origin")
        );

        let mut missing = home;
        missing.remove("origin");
        missing.remove("sec-fetch-site");
        assert!(require_home_token_context(data_dir.path(), &missing)
            .unwrap_err()
            .to_string()
            .contains("missing same-origin provenance"));
    }

    #[test]
    fn launch_grant_accepts_explicit_parent_gateway_signer() {
        let parent = tempfile::tempdir().unwrap();
        let child = tempfile::tempdir().unwrap();
        let (_parent_key, parent_did) =
            elastos_identity::load_or_create_did(parent.path()).unwrap();
        let (_child_key, child_did) = elastos_identity::load_or_create_did(child.path()).unwrap();
        assert_ne!(parent_did, child_did);

        let context = local_home_launch_token_context(parent.path()).unwrap();
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
            HomeLaunchOriginPolicy::InternalShell,
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
            parent_did.clone(),
            child.path(),
            HomeLaunchOriginPolicy::InternalShell,
        )
        .unwrap();
        assert_eq!(parent_result.context.principal_id, context.principal_id);

        let mut browser_headers = headers;
        browser_headers.insert(axum::http::header::ORIGIN, "null".parse().unwrap());
        assert!(require_home_launch_token_for_any_from_expected_did(
            child.path(),
            &browser_headers,
            &["marketplace"],
            None,
            parent_did,
            child.path(),
            HomeLaunchOriginPolicy::InternalShell,
        )
        .unwrap_err()
        .to_string()
        .contains("must not carry browser provenance"));
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
            HomeLaunchOriginPolicy::InternalShell,
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
            HomeLaunchOriginPolicy::InternalShell,
        )
        .unwrap();
        assert_eq!(parent_auth_result.context.session_id, "auth:alice");
    }
}
