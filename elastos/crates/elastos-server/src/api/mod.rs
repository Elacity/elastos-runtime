//! HTTP API module
//!
//! This module provides the HTTP API for the ElastOS runtime:
//! - Session authentication via bearer tokens
//! - Capability request/grant/deny flow
//! - Health and status endpoints

pub mod access_grant;
pub mod auth_gateway;
pub mod browser_capsules;
pub mod browser_engine_protocol;
pub mod browser_sessions;
pub mod buy_authority;
pub(crate) mod capsule_inventory;
pub(crate) mod capsule_watchdog;
pub mod chain_tx;
pub mod content_index;
pub mod creator;
pub mod gateway;
pub mod handlers;
pub mod market_reads;
pub mod media_authority;
pub mod middleware;
pub mod mint_authority;
pub mod object_authority;
pub mod owned_ledger;
pub mod rights_authority;
pub mod routes;
pub mod server;
pub(crate) mod session_bounds;
pub(crate) mod session_lifecycle;
pub mod trade_authority;
pub mod viewer_gateway;
pub mod viewer_media;
pub mod viewer_object;
pub mod viewer_open;
pub mod wallet_signer;

/// Attach a Home launch token to a viewer launch route, in the URL **fragment**.
///
/// A fragment is never transmitted to any server. That keeps the token out of `Referer` on every
/// subresource the viewer loads, out of the gateway access log, and out of shared browser history
/// — none of which is true of the `?…&home_token=…` query pair these routes used to carry, even
/// though the token is a bearer credential for the whole viewer session.
///
/// It costs the readers nothing: both viewer capsules already send the token as the
/// `x-elastos-home-token` HEADER on every call and pull their bytes through `fetch` into blob URLs
/// (never a direct `<img src>`/`<video src>` against a credentialed URL), so the query string was
/// pure delivery, not a requirement of the media/object read paths. The session id stays in the
/// query where it is: it is the per-open capability those routes are scoped by, and unlike the
/// token it is single-asset, short-lived, and revoked by `/close`.
///
/// Same shape as `gateway_home_runtime::append_home_launch_token_to_route`, which is how Home's
/// own app launches have always delivered the token; this brings the viewer surfaces onto it.
pub(crate) fn viewer_route_with_launch_token(route: &str, token: &str) -> String {
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("home_token", token)
        .finish();
    format!("{route}#{fragment}")
}

// One process-wide lock serializing tests that mutate the shared `ELASTOS_DDRM_*` environment
// (rights/buy/mint/owned-ledger authority modules). These vars are process-global, so per-module
// locks only serialize a module against ITSELF — a reader in one module could still observe another
// module's mid-test mutation and fail closed. A single shared lock closes that cross-module race
// (the same nondeterministic class the trusted-auth-env guard fixed). Poison is ignored: the
// guarded unit is `()` with no invariant state.
#[cfg(test)]
pub(crate) fn ddrm_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static DDRM_ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    DDRM_ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod launch_token_delivery_tests {
    use super::*;

    /// The two URL-borne shapes a launch token must never take. Assembled at runtime so this
    /// module's own source does not trip the scanner below.
    fn url_borne_token_shapes() -> Vec<String> {
        ["?", "&"]
            .iter()
            .map(|sep| format!("{sep}home_token="))
            .collect()
    }

    /// The composed viewer launch route: session in the query (a per-open capability), token in
    /// the fragment (a bearer credential that must never reach a server as part of a URL).
    #[test]
    fn viewer_launch_routes_carry_the_token_only_in_the_fragment() {
        for route in [
            viewer_media::media_play_route("sess-abc"),
            viewer_object::object_view_route("sess-abc"),
        ] {
            let launched = viewer_route_with_launch_token(&route, "tok-secret");
            let (before_fragment, fragment) = launched
                .split_once('#')
                .expect("the launch route carries a fragment");
            assert!(
                !before_fragment.contains("home_token"),
                "the token must not appear before the fragment: {launched}"
            );
            for url_borne in url_borne_token_shapes() {
                assert!(
                    !launched.contains(&url_borne),
                    "no URL-borne token pair ({url_borne}): {launched}"
                );
            }
            assert_eq!(fragment, "home_token=tok-secret");
            assert!(
                before_fragment.contains("?session=sess-abc"),
                "the session id stays in the query: {launched}"
            );
        }
        // A token with URL-significant bytes must be encoded, not spliced in raw — otherwise a
        // token containing `&` or `#` would silently truncate or forge fragment pairs.
        let hostile = viewer_route_with_launch_token("/apps/x/?session=s", "a&b=c#d e");
        assert_eq!(hostile, "/apps/x/?session=s#home_token=a%26b%3Dc%23d+e");
    }

    /// Regression guard for the delivery itself. The six viewer-launch sites used to splice
    /// `&home_token=` onto the route, putting a bearer credential into `Referer`, the access log
    /// and shared history. Nothing in the gateway may reintroduce that shape.
    #[test]
    fn no_gateway_source_builds_a_url_borne_launch_token() {
        let api_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api");
        let mut offenders: Vec<String> = Vec::new();
        let mut scanned = 0usize;
        let mut stack = vec![api_dir];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("api source dir is readable") {
                let path = entry.expect("readable dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                // The gateway test suite deliberately exercises the refused query form.
                let name = path.to_string_lossy().to_string();
                if name.contains("gateway_tests") {
                    continue;
                }
                scanned += 1;
                let source = std::fs::read_to_string(&path).expect("source is readable");
                for (index, line) in source.lines().enumerate() {
                    let line = line.trim_start();
                    if line.starts_with("//") {
                        continue;
                    }
                    if url_borne_token_shapes()
                        .iter()
                        .any(|shape| line.contains(shape))
                    {
                        offenders.push(format!("{}:{}: {}", name, index + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            scanned >= 20,
            "expected to scan the api tree, saw {scanned}"
        );
        assert!(
            offenders.is_empty(),
            "launch tokens must ride the URL fragment (viewer_route_with_launch_token), never a \
             query pair — a query-borne token leaks into Referer, access logs and history: \
             {offenders:#?}"
        );
    }
}
