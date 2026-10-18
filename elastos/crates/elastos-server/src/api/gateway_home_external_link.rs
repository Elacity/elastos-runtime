use super::*;
use serde_json::json;

const HOME_EXTERNAL_LINK_REQUEST_SCHEMA: &str = "elastos.home.external-link.request/v1";
const HOME_EXTERNAL_LINK_RESULT_SCHEMA: &str = "elastos.home.external-link.result/v1";
/// Longest web address Home shows and opens, in bytes.
const MAX_EXTERNAL_LINK_BYTES: usize = 2048;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeExternalLinkRequest {
    schema: String,
    app_token: String,
    url: String,
}

#[derive(Debug, PartialEq, Eq)]
struct HomeExternalLinkApp {
    id: String,
    title: String,
}

/// Home asks which app wants to open a web link and what exactly would open
/// (#430). The app comes from its signed launch under the same Home session,
/// never from the message, and the address is checked, not fetched. Home shows
/// both and opens the returned address only when the person chooses Open link.
pub(super) async fn home_external_link(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeExternalLinkRequest>,
) -> Response {
    if input.schema != HOME_EXTERNAL_LINK_REQUEST_SCHEMA {
        return (
            StatusCode::BAD_REQUEST,
            "unsupported Home external-link request schema",
        )
            .into_response();
    }
    let app = match home_external_link_app(&state.data_dir, &headers, &input.app_token) {
        Ok(app) => app,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let url = match home_browser_origin(&headers)
        .and_then(|home_origin| exact_external_web_link(&input.url, &home_origin))
    {
        Ok(url) => url,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    Json(json!({
        "schema": HOME_EXTERNAL_LINK_RESULT_SCHEMA,
        "app_id": app.id,
        "app_title": app.title,
        "url": url,
    }))
    .into_response()
}

/// The app a carried launch token names, accepted only when Home launched it
/// under the caller's own Home authority (principal, session, proof binding and
/// grant all match).
fn home_external_link_app(
    data_dir: &std::path::Path,
    headers: &HeaderMap,
    app_token: &str,
) -> anyhow::Result<HomeExternalLinkApp> {
    let home = require_home_token_context(data_dir, headers)?;
    let targets: Vec<_> = home_launch_targets(data_dir)
        .into_iter()
        .filter(|target| target.target != HOME_GUI_SHELL_ID)
        .collect();
    let actors: Vec<&str> = targets
        .iter()
        .map(|target| target.viewer.as_deref().unwrap_or(&target.target))
        .collect();
    let app = require_carried_home_launch_token(data_dir, app_token, &actors)?;
    if app.launch_context.authority_actor != HOME_CAPSULE_ID || app.context != home {
        anyhow::bail!("Home and app launch authority mismatch");
    }
    let target = targets
        .into_iter()
        .find(|target| {
            target.target == app.launch_context.selected_resource
                && target.viewer.as_deref().unwrap_or(&target.target)
                    == app.launch_context.executable_actor
        })
        .ok_or_else(|| anyhow::anyhow!("Home did not launch this app"))?;
    Ok(HomeExternalLinkApp {
        title: target.viewer_title.unwrap_or(target.title),
        id: target.target,
    })
}

/// The one address Home may show and open: an absolute http(s) URL with a
/// host, no credentials and no control characters, in canonical ASCII form.
/// Home's own pages open inside Home, never in a browser tab.
fn exact_external_web_link(raw: &str, home_origin: &str) -> anyhow::Result<String> {
    if raw.is_empty()
        || raw.len() > MAX_EXTERNAL_LINK_BYTES
        || raw.trim() != raw
        || raw.chars().any(char::is_control)
    {
        anyhow::bail!("invalid web link");
    }
    let parsed = url::Url::parse(raw).map_err(|_| anyhow::anyhow!("invalid web link"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("only http and https links open outside ElastOS");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("web links with credentials do not open");
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        anyhow::bail!("invalid web link");
    }
    if parsed.origin().ascii_serialization() == home_origin {
        anyhow::bail!("Home's own pages open inside Home");
    }
    let href = String::from(parsed);
    if href.len() > MAX_EXTERNAL_LINK_BYTES || !href.is_ascii() {
        anyhow::bail!("invalid web link");
    }
    Ok(href)
}

#[cfg(test)]
mod tests {
    use super::exact_external_web_link;

    const HOME: &str = "http://localhost:61180";

    fn check(raw: &str) -> anyhow::Result<String> {
        exact_external_web_link(raw, HOME)
    }

    #[test]
    fn only_exact_http_links_leave_elastos() {
        assert_eq!(
            check("https://Example.com/a b?q=1#x").unwrap(),
            "https://example.com/a%20b?q=1#x"
        );
        assert_eq!(
            check("http://xn--bcher-kva.example/").unwrap(),
            "http://xn--bcher-kva.example/"
        );
        assert_eq!(
            check("https://bücher.example/").unwrap(),
            "https://xn--bcher-kva.example/"
        );
        // Another port or scheme on the same host is not Home.
        assert!(check("http://localhost:8080/").is_ok());
        assert!(check("https://localhost:61180/").is_ok());
        for refused in [
            "",
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "elastos://doc",
            "blob:https://example.com/id",
            "ftp://example.com/",
            "https://user:pass@example.com/",
            "https://user@example.com/",
            "https://",
            "https://example.com/\u{0}",
            "https://example.com/a\nb",
            "https://example.com/a\tb",
            " https://example.com/",
            "https://example.com/ ",
            "/relative",
            "not a url",
            "http://localhost:61180/apps/home/",
            "HTTP://LOCALHOST:61180/api/apps/home/summary",
        ] {
            assert!(check(refused).is_err(), "{refused:?} must not open");
        }
        let longest = format!("https://example.com/{}", "a".repeat(2048 - 20));
        assert_eq!(longest.len(), 2048);
        assert_eq!(check(&longest).unwrap(), longest);
        assert!(check(&format!("{longest}a")).is_err());
        // Normalisation may not grow an address past the bound.
        let grows = format!("https://example.com/{}x", " ".repeat(2048 - 21));
        assert_eq!(grows.len(), 2048);
        assert!(check(&grows).is_err());
    }
}
