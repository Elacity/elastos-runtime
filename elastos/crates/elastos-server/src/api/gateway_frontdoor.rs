//! Admission at the gateway boundary, before an extractor can read a body.
use super::*;
use axum::http::{header::HOST, Method};
use std::collections::BTreeSet;

#[derive(Clone, Default)]
pub(super) struct GatewayFrontDoor {
    authorities: BTreeSet<String>,
}

impl GatewayFrontDoor {
    pub(super) fn load(data_dir: &FsPath, api_url: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(api_url)?;
        let mut authorities = BTreeSet::from([canonical_authority(url.authority())?]);
        if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
            let port = url.port_or_known_default().unwrap_or(80);
            for host in ["localhost", "127.0.0.1", "[::1]"] {
                authorities.insert(canonical_authority(&format!("{host}:{port}"))?);
            }
        }
        let config = match std::fs::read_to_string(data_dir.join("config.toml")) {
            Ok(config) => config.parse::<toml::Table>()?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(err) => return Err(err.into()),
        };
        if let Some(hosts) = config.get("gateway_allowed_hosts") {
            let hosts = hosts
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("gateway_allowed_hosts must be an array"))?;
            if hosts.len() > 32 {
                anyhow::bail!("gateway_allowed_hosts exceeds 32 names");
            }
            for host in hosts {
                let host = host
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("gateway host must be text"))?;
                authorities.insert(canonical_authority(host)?);
            }
        }
        Ok(Self { authorities })
    }

    fn allows_host(&self, headers: &HeaderMap) -> bool {
        single_header(headers, HOST.as_str())
            .and_then(|value| canonical_authority(value).ok())
            .is_some_and(|host| self.authorities.contains(&host))
    }
}

fn canonical_authority(value: &str) -> anyhow::Result<String> {
    if value.is_empty() || value.trim() != value || value.contains(['/', '@', '?', '#']) {
        anyhow::bail!("invalid gateway authority");
    }
    let authority = value.parse::<axum::http::uri::Authority>()?;
    let host = authority.host().to_ascii_lowercase();
    if host.ends_with('.')
        || host.contains('%')
        || authority.as_str() != authority.host() && authority.port_u16().is_none()
    {
        anyhow::bail!("invalid gateway authority");
    }
    Ok(match authority.port_u16() {
        Some(80) | None => host,
        Some(port) => format!("{host}:{port}"),
    })
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() || value.is_empty() {
        return None;
    }
    Some(value)
}

fn same_gateway_origin(headers: &HeaderMap) -> bool {
    let Some(host) = single_header(headers, "host") else {
        return false;
    };
    let scheme = if request_uses_tls(headers) {
        "https"
    } else {
        "http"
    };
    let Ok(expected) = url::Url::parse(&format!("{scheme}://{host}")) else {
        return false;
    };
    let origin = expected.origin().ascii_serialization();
    if headers.contains_key("origin") {
        return single_header(headers, "origin") == Some(origin.as_str());
    }
    if let Some(referer) = single_header(headers, "referer") {
        return url::Url::parse(referer).is_ok_and(|url| {
            url.origin() == expected.origin()
                && url.username().is_empty()
                && url.password().is_none()
        });
    }
    single_header(headers, "sec-fetch-site") == Some("same-origin")
}

/// These objects are public read-only data. API authority has a separate gate.
fn public_read(path: &str) -> bool {
    matches!(
        path,
        "/healthz"
            | "/install.sh"
            | "/release.json"
            | "/release-head.json"
            | "/.well-known/elastos/site-head.json"
    ) || path.starts_with("/artifacts/")
        || path == "/home"
        || path.starts_with("/home/")
        || path.starts_with("/apps/")
        || !path.starts_with("/api/") && !path.starts_with("/.well-known/")
}

/// Only the Home sign-in journey can begin without an existing launch.
fn sign_in_route(path: &str, method: &Method) -> bool {
    matches!(
        (method.as_str(), path),
        ("GET", "/api/auth/passkey/status")
            | (
                "POST",
                "/api/auth/passkey/register/begin"
                    | "/api/auth/passkey/register/complete"
                    | "/api/auth/passkey/authenticate/begin"
                    | "/api/auth/passkey/authenticate/complete"
            )
    )
}

fn runtime_operator_route(path: &str, method: &Method) -> bool {
    let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), parts.as_slice()) {
        ("POST", ["api", "apps", "browser", "pages", page, "input"])
        | ("POST", ["api", "apps", "browser", "pages", page, "operator-requests"]) => {
            !page.is_empty()
        }
        ("GET", ["api", "apps", "browser", "pages", page, "operator-requests", id])
        | (
            "POST",
            ["api", "apps", "browser", "pages", page, "operator-requests", id, "inspect" | "detach"],
        ) => !page.is_empty() && !id.is_empty(),
        _ => false,
    }
}

pub(super) fn public_publisher_bootstrap(
    data_dir: &FsPath,
    path: &str,
    query: Option<&str>,
) -> bool {
    if path != "/.well-known/elastos/carrier-bootstrap.json" {
        return false;
    }
    let publisher = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .any(|(key, value)| key == "role" && value == "publisher");
    publisher
        && std::fs::read_to_string(data_dir.join("config.toml"))
            .ok()
            .and_then(|config| config.parse::<toml::Table>().ok())
            .and_then(|config| {
                config
                    .get("gateway_public_publisher_bootstrap")
                    .and_then(toml::Value::as_bool)
            })
            == Some(true)
}

pub(super) async fn gateway_admission(
    State(state): State<GatewayState>,
    Extension(frontdoor): Extension<GatewayFrontDoor>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let headers = request.headers();
    let path = request.uri().path();
    let method = request.method();
    let refused = || {
        (
            StatusCode::FORBIDDEN,
            "Gateway request requires an admitted host and caller",
        )
            .into_response()
    };
    if !frontdoor.allows_host(headers)
        || headers.contains_key("origin") && single_header(headers, "origin").is_none()
    {
        return refused();
    }
    let origin = single_header(headers, "origin");
    if origin.is_some_and(|origin| origin != "null") && !same_gateway_origin(headers) {
        return refused();
    }
    // A browser preflight carries header names, never the launch-token value.
    // Its response advertises the protocol; admission of the request still
    // requires the signed launch before any request body can be read.
    if method == Method::OPTIONS {
        let requested_method = single_header(headers, "access-control-request-method");
        let requested_headers =
            single_header(headers, "access-control-request-headers").unwrap_or("");
        if origin == Some("null")
            && requested_headers
                .split(',')
                .any(|h| h.trim().eq_ignore_ascii_case("x-elastos-home-token"))
            && matches!(
                requested_method,
                Some("GET" | "POST" | "PUT" | "DELETE" | "HEAD")
            )
            && path.starts_with("/api/")
        {
            let mut response = StatusCode::NO_CONTENT.into_response();
            apply_capsule_cors_headers(response.headers_mut(), &HeaderValue::from_static("null"));
            response.headers_mut().insert(
                "access-control-allow-methods",
                HeaderValue::from_static("GET, POST, PUT, DELETE, HEAD"),
            );
            response.headers_mut().insert(
                "access-control-allow-headers",
                HeaderValue::from_static("content-type,x-elastos-home-token,last-event-id,x-elastos-upload-offset,x-elastos-recovery-terminal,if-match,if-none-match,range"),
            );
            return response;
        }
        return refused();
    }
    let read = method == Method::GET || method == Method::HEAD;
    if path.starts_with("/api/apps/home/")
        && gateway_home_system::require_home_active_shell_wallet_authority(&state.data_dir, headers)
            .is_err()
    {
        return refused();
    }
    let public = read
        && (public_read(path)
            || public_publisher_bootstrap(&state.data_dir, path, request.uri().query()));
    let sign_in = sign_in_route(path, method) && same_gateway_origin(headers);
    let ticket =
        read && gateway_home_terminal::admitted_terminal_ticket(path, request.uri().query()).await;
    let runtime_session = runtime_operator_route(path, method)
        && headers.contains_key("authorization")
        && !headers.contains_key("origin")
        && !headers.contains_key("referer")
        && !headers.contains_key("sec-fetch-site");
    if runtime_session {
        if single_header(headers, "authorization").is_none() {
            return refused();
        }
        if let Err(response) =
            gateway_browser::gateway_browser_operator::admit_runtime_session(&state, headers).await
        {
            return response;
        }
    }
    if !public
        && !sign_in
        && !ticket
        && !runtime_session
        && gateway_home_token::require_gateway_launch(&state.data_dir, headers).is_err()
    {
        return refused();
    }
    if sign_in && method == Method::POST {
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|peer| peer.0);
        if let Err(response) =
            super::super::auth_gateway::admit_passkey_begin(&state.data_dir, peer, path)
        {
            return response;
        }
    }
    let opaque = origin == Some("null");
    let mut response = next.run(request).await;
    if opaque && (public || !sign_in) {
        apply_capsule_cors_headers(response.headers_mut(), &HeaderValue::from_static("null"));
        response.headers_mut().insert(
            "access-control-expose-headers",
            HeaderValue::from_static(
                "etag,content-disposition,x-elastos-request-id,x-elastos-transfer-receipt",
            ),
        );
    }
    response
}
