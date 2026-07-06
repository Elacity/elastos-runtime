//! The mandates surface for the ElastOS home gateway: list, receipt, revoke, issue.
//!
//! The `mandates` capsule (see `capsules/mandates/`) opens as an app window in the existing
//! shell. It needs LIVE mandate data, and the gateway is the surface the shell talks to. But the
//! standing-grant registry and the durable audit chain live on the API server's
//! [`crate::api::handlers::capability::CapabilityState`], not on the gateway's `GatewayState`.
//!
//! Rather than churn the ~40 `GatewayState` construction sites, this module is a small axum
//! sub-router carrying its own [`MandateApiState`]. The serve path hoists ONE shared
//! `standing_service` + `capability_manager` into [`crate::server_infra::ServerInfrastructure`] and
//! hands the SAME handles to both the API server and (via the supervisor) this sub-router — so the
//! shell reads exactly the registry the API server issues mandates into. The router is merged into
//! the gateway only when both handles are present (see [`super::gateway_server::start_gateway_server`]).
//!
//! Every route is gated by the same home-launch token as every other shell app — the mandates app
//! must be the launched capsule. The two GET routes are strictly read-only. The surface carries
//! two mutations. REVOKE is the kill switch, fail-safe: it only ever REMOVES authority (P11/P16).
//! ISSUE (Sprint 15) mints a mandate — authority-GRANTING, so its trust argument is explicit: the
//! home-launch token is minted by the runtime's own signing key when the SHELL launches the app,
//! and the shell is already the runtime's grant root (the API server's issue endpoint sits behind
//! the consent-broker/shell gate for the same reason; G-M3 — the shell can issue any mandate
//! anyway). The gateway surface adds NO new authority tier; it re-homes the shell's own verb into
//! the shell's own app window, and delegates to the SAME shared mint path with every fail-closed
//! guard intact (action whitelist, non-empty methods, AUD-5 overbroad-wildcard refusal, agent-key
//! validation, durable-before-visible issuance).
//!
//! All three verbs delegate to the API server's own shared helpers
//! ([`mandate_cards`](crate::api::handlers::capability::mandate_cards),
//! [`revoke_mandate`](crate::api::handlers::capability::revoke_mandate),
//! [`issue_mandate`](crate::api::handlers::capability::issue_mandate)) so neither the liveness
//! invariant, the fail-closed revoke order, nor the mint guards can drift between surfaces (P12:
//! one honest source of truth; a revoked mandate never renders "Live"; a mandate that cannot be
//! durably recorded is not issued).

use super::*;

use elastos_runtime::capability::token::TokenId;
use elastos_runtime::capability::CapabilityManager;
use std::path::PathBuf;

/// State for the read-only mandates sub-router. Cloneable (all `Arc`) so axum can share it.
#[derive(Clone)]
pub(crate) struct MandateApiState {
    /// The SAME standing-grant registry the API server issues mandates into.
    pub(crate) standing_service: Arc<elastos_runtime::capability::intent::StandingGrantService>,
    /// Owns the durable audit chain (receipt export) and the token-revocation liveness check.
    pub(crate) capability_manager: Arc<CapabilityManager>,
    /// Home-launch-token trust root (the operator's 0600 files under the data dir).
    pub(crate) data_dir: PathBuf,
    /// The SAME meter+ledger the executor's pay gate enforces with (Sprint 31) — the Money
    /// panel's read/provision/reconcile surface. `None` ⇒ pay unwired ⇒ those routes answer 503.
    pub(crate) pay_rail: Option<crate::api::server::PayRail>,
}

/// The mandates capsule id — must match `capsules/mandates/capsule.json`'s `name`.
const MANDATES_CAPSULE_ID: &str = "mandates";

/// GET /api/apps/mandates/standing-grants — the shell app's live mandate list.
///
/// Delegates to [`crate::api::handlers::capability::mandate_cards`] — the ONE shared projection the
/// API server also serves — so the liveness invariant (a revoked/expired/epoch-killed mandate never
/// renders "Live") can never drift between the two surfaces.
async fn mandates_list(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    Json(crate::api::handlers::capability::mandate_cards(
        &state.standing_service,
        &state.capability_manager,
    )
    .await)
    .into_response()
}

/// GET /api/apps/mandates/agent-state — the operator's view of durable agent state (Sprint 18).
///
/// Read-only, OPERATOR-scoped: it spans every principal because the caller is the shell (the
/// runtime's grant root, gated by the same home-launch token as the mandate list), the trust level
/// that already sees every mandate. It is NOT an agent path — agents remain isolated by the
/// per-principal `get_agent_state`; this only lets the OWNER watch what its agents have written
/// under their mandates. The value shown is the `input_hash` COMMITMENT (labelled as such by the
/// UI), never content bytes — the store holds no payload.
async fn agent_state_list(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    match crate::agent_store::list_agent_state(&state.data_dir) {
        Ok(entries) => Json(serde_json::json!({ "entries": entries })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not read agent state: {e}"),
        )
            .into_response(),
    }
}

/// GET /api/apps/mandates/mandate/:token_id/receipt — the portable per-mandate receipt.
///
/// Mirrors [`crate::api::handlers::capability::mandate_receipt`]: read-only over the durable chain;
/// `404` when the token has no durable records (absence reported, never fabricated).
async fn mandate_receipt(
    State(state): State<MandateApiState>,
    Path(token_id): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    let token_id = match TokenId::from_hex(token_id.trim()) {
        Ok(id) => id.to_string(),
        Err(e) => return (StatusCode::BAD_REQUEST, format!("invalid token id: {e}")).into_response(),
    };
    match state
        .capability_manager
        .audit_log()
        .export_mandate_receipt_for_capability(&token_id)
    {
        Some(receipt) => Json(receipt).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            format!("no durable audit records for mandate {token_id}"),
        )
            .into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeBody {
    /// The mandate's token id (the same id the card carries).
    grant_id: String,
}

/// POST /api/apps/mandates/standing-grants/revoke — the operator's kill switch, from the shell.
///
/// Delegates to [`crate::api::handlers::capability::revoke_mandate`] — the SAME fail-closed kill
/// path the API server uses (signed `CapabilityRevoke` durably attested BEFORE the envelope dies;
/// an unattestable revoke ABORTS loudly). Returns `{revoked: bool}`: `true` iff a live envelope was
/// killed by THIS call (idempotent — an already-dead or unknown mandate reads `false`, honestly).
/// This is the surface's only mutation, and it exclusively REMOVES authority.
async fn mandate_revoke(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
    Json(body): Json<RevokeBody>,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    match crate::api::handlers::capability::revoke_mandate(
        &state.standing_service,
        &state.capability_manager,
        &body.grant_id,
        "standing grant revoked via shell mandates app",
    )
    .await
    {
        Ok(revoked) => Json(serde_json::json!({ "revoked": revoked })).into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

/// POST /api/apps/mandates/standing-grants/issue — grant a mandate from the shell app.
///
/// Delegates to [`crate::api::handlers::capability::issue_mandate`] — the SAME fail-closed mint
/// path the API server uses (action whitelist, non-empty methods, AUD-5 overbroad-wildcard
/// refusal, agent-key validation, durable-before-visible issuance) — so the guards cannot drift.
/// Authority-granting, therefore shell-only twice over: the home-token gate here AND the launch
/// token itself only exists because the shell (the runtime's grant root, G-M3) opened the app.
async fn mandate_issue(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
    Json(body): Json<crate::api::handlers::capability::IssueStandingGrantInput>,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    // Least privilege (Sprint 15 council, P16): the web mint surface is DELIBERATELY narrower than
    // the raw API — `admin` mandates are minted from the CLI/consent-broker API only, never from
    // this iframe. Enforced server-side (not just hidden in the form) so an XSS in the frame that
    // POSTs `action:"admin"` with the in-URL token is still refused. The shared helper keeps admin
    // for the API/CLI path; this narrowing is the gateway surface's own.
    if body.action.trim().eq_ignore_ascii_case("admin") {
        return (
            StatusCode::FORBIDDEN,
            "admin mandates are minted from the CLI, not the shell app".to_string(),
        )
            .into_response();
    }
    // G-M4 default-bound (Sprint 20): the web mint surface REQUIRES an agent key — an unbound
    // (capsule-string-only) mandate lets ANY key acting as the capsule act, weak attribution that
    // should be a deliberate operator choice, not a one-click default. Enforced server-side (not
    // just prompted in the form) so the narrower posture holds even against an XSS in the frame.
    // Unbound mandates remain available on the CLI/consent-broker API for the trusted operator
    // (G-M3) — this narrowing, like the admin refusal above, is the gateway surface's own.
    if body
        .agent_pubkey
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            "a mandate granted from the shell must bind an agent key (unbound mandates are \
             CLI-only); paste the agent's 64-hex ed25519 public key"
                .to_string(),
        )
            .into_response();
    }
    match crate::api::handlers::capability::issue_mandate(
        &state.standing_service,
        &state.capability_manager,
        body,
    )
    .await
    {
        Ok(out) => Json(out).into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

/// GET /api/apps/mandates/money — the Money panel's one-call read (Sprint 31): every provisioned
/// budget (with held-unconfirmed `pending_units` distinct from confirmed spend), the
/// reconciliation work list, and the poisoned flag — a READ-ONLY projection of the same meter and
/// ledger the pay gate enforces with (delegating to the shared
/// [`money_overview`](crate::api::handlers::capability::money_overview), so the two surfaces can
/// never drift). 503 when pay is unwired — absence reported, never an empty fabrication.
async fn money_view(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    let Some(rail) = &state.pay_rail else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired".to_string(),
        )
            .into_response();
    };
    Json(crate::api::handlers::capability::money_overview(
        &rail.meter,
        &rail.ledger,
    ))
    .into_response()
}

/// POST /api/apps/mandates/spend-budget — provision (or re-set) a capsule's money cap from the
/// shell app. Authority-granting like `issue`, gated the same way (home-launch token = the shell
/// grant root, G-M3), and delegating to the ONE shared provisioning path
/// ([`set_spend_budget_core`](crate::api::handlers::capability::set_spend_budget_core)) so every
/// fail-closed guard (durable-only, slug bound, apply-then-attest with true rollback, loud double
/// failure) holds identically on both surfaces.
async fn money_set_budget(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
    Json(body): Json<crate::api::handlers::capability::SetSpendBudgetInput>,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    let Some(rail) = &state.pay_rail else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired — a spend budget nothing enforces is refused".to_string(),
        )
            .into_response();
    };
    match crate::api::handlers::capability::set_spend_budget_core(
        &rail.meter,
        Some(&rail.ledger),
        state.capability_manager.audit_log(),
        body,
    ) {
        Ok(out) => Json(out).into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

/// POST /api/apps/mandates/payments/reconcile — resolve ONE indeterminate payment against the
/// rail's verdict, from the shell app. Delegates to the ONE shared reconciliation path
/// ([`reconcile_payment_core`](crate::api::handlers::capability::reconcile_payment_core)):
/// exactly-once resolve, refund only on durable Ok, structured 404/409/503, chain attestation —
/// identical on both surfaces by construction.
async fn money_reconcile(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
    Json(body): Json<crate::api::handlers::capability::ReconcilePaymentInput>,
) -> axum::response::Response {
    if let Err(err) = super::require_home_launch_token(&state.data_dir, &headers, MANDATES_CAPSULE_ID)
    {
        return mandate_auth_error(err);
    }
    let Some(rail) = &state.pay_rail else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired".to_string(),
        )
            .into_response();
    };
    match crate::api::handlers::capability::reconcile_payment_core(
        &rail.ledger,
        &rail.meter,
        state.capability_manager.audit_log(),
        body,
    ) {
        Ok(out) => Json(out).into_response(),
        Err((status, msg)) => (status, msg).into_response(),
    }
}

/// A failed home-token gate reads as `401` — the app was not launched through the shell.
fn mandate_auth_error(err: anyhow::Error) -> axum::response::Response {
    (StatusCode::UNAUTHORIZED, err.to_string()).into_response()
}

/// Build the mandates sub-router (read + the revoke kill switch), erased over its own state so the
/// gateway can `.merge()` it without disturbing `GatewayState`.
pub(crate) fn mandate_router(state: MandateApiState) -> Router {
    Router::new()
        .route("/api/apps/mandates/standing-grants", get(mandates_list))
        .route("/api/apps/mandates/agent-state", get(agent_state_list))
        .route(
            "/api/apps/mandates/mandate/:token_id/receipt",
            get(mandate_receipt),
        )
        .route(
            "/api/apps/mandates/standing-grants/revoke",
            post(mandate_revoke),
        )
        .route(
            "/api/apps/mandates/standing-grants/issue",
            post(mandate_issue),
        )
        // The Money panel (Sprint 31): read the budgets+work-list projection, provision a cap,
        // resolve an indeterminate payment — all over the same home-launch-token gate.
        .route("/api/apps/mandates/money", get(money_view))
        .route("/api/apps/mandates/spend-budget", post(money_set_budget))
        .route(
            "/api/apps/mandates/payments/reconcile",
            post(money_reconcile),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use elastos_runtime::capability::token::TokenConstraints;
    use elastos_runtime::capability::{
        Action, CapabilityStore, ResourceId, StandingGrantService,
    };
    use elastos_runtime::primitives::audit::AuditLog;
    use elastos_runtime::primitives::metrics::MetricsManager;
    use tower::ServiceExt;

    const HOME_TOKEN_HEADER: &str = "x-elastos-home-token";

    /// A fresh manager + the standing service backed by it (the SAME registry, as in serve).
    fn manager_and_service() -> (Arc<CapabilityManager>, Arc<StandingGrantService>) {
        let audit_log = Arc::new(AuditLog::new());
        let store = Arc::new(CapabilityStore::new());
        let metrics = Arc::new(MetricsManager::new());
        let capability_manager = Arc::new(CapabilityManager::new(store, audit_log, metrics));
        let standing_service = Arc::new(capability_manager.standing_grant_service());
        (capability_manager, standing_service)
    }

    fn state_for(dir: &std::path::Path) -> MandateApiState {
        let (capability_manager, standing_service) = manager_and_service();
        MandateApiState {
            standing_service,
            capability_manager,
            data_dir: dir.to_path_buf(),
            pay_rail: None,
        }
    }

    /// Helper: POST a JSON body to a route with optional home token, return the response.
    async fn post_json(
        app: Router,
        uri: &str,
        token: Option<String>,
        body: &str,
    ) -> axum::http::Response<Body> {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header(HOME_TOKEN_HEADER, t);
        }
        app.oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    /// The MINT is gated like everything else: no home-launch token ⇒ 401 AND nothing is minted —
    /// an unauthenticated caller can never grant an agent authority.
    #[tokio::test]
    async fn issue_route_requires_home_launch_token_and_mints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let app = mandate_router(state.clone());
        let resp = post_json(
            app,
            "/api/apps/mandates/standing-grants/issue",
            None,
            r#"{"capsule":"vm-agent","resource":"elastos://mail/send","action":"execute","methods":["send"],"ttl_secs":3600}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            state.standing_service.list().is_empty(),
            "an unauthenticated issue must mint NOTHING"
        );
    }

    /// Grant from the shell: a valid issue over the route mints a LIVE mandate in the SHARED
    /// registry, visible on the list surface, with the returned grant id.
    #[tokio::test]
    async fn issue_route_mints_a_live_mandate_in_the_shared_registry() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        // The web surface grants BOUND mandates (G-M4) — pass a REAL, non-weak agent key.
        let agent = hex::encode(
            ed25519_dalek::SigningKey::generate(&mut rand::thread_rng())
                .verifying_key()
                .to_bytes(),
        );
        let resp = post_json(
            mandate_router(state.clone()),
            "/api/apps/mandates/standing-grants/issue",
            Some(token_hdr),
            &format!(
                r#"{{"capsule":"vm-agent","resource":"elastos://mail/send","action":"execute","methods":["send"],"ttl_secs":3600,"agent_pubkey":"{agent}"}}"#
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let out: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let grant_id = out["grant_id"].as_str().expect("grant id returned");
        assert_eq!(out["token_id"], grant_id, "grant id IS the backing token id");
        assert!(
            state.standing_service.is_active(grant_id),
            "the minted mandate is LIVE in the shared registry"
        );
        let cards = crate::api::handlers::capability::mandate_cards(
            &state.standing_service,
            &state.capability_manager,
        )
        .await;
        let card = cards
            .mandates
            .iter()
            .find(|c| c.token_id == grant_id)
            .expect("the minted mandate is listed");
        assert!(card.active);
        assert_eq!(card.capsule, "vm-agent");
        assert_eq!(card.methods, vec!["send".to_string()]);
        // The card SURFACES the binding (G-M4): bound = true, and the agent key round-trips.
        assert!(card.agent_bound, "the card shows the mandate is agent-bound");
        assert_eq!(card.agent_pubkey.as_deref(), Some(agent.as_str()));
    }

    /// The gateway mint enforces the SAME fail-closed guards as the API server (shared helper):
    /// unknown action 400, empty methods 400, AUD-5 bare scheme wildcard 403, malformed agent key
    /// 400 — and NONE of them mint anything.
    #[tokio::test]
    async fn issue_route_enforces_the_shared_guards() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        // A valid 64-hex agent key so downstream guards (wildcard, key-format) are the ones tested,
        // not the new G-M4 bound-required check (which fires first when the key is absent).
        let k = "\"agent_pubkey\":\"".to_string() + &"ab".repeat(32) + "\"";
        let cases: Vec<(String, StatusCode)> = vec![
            (
                format!(r#"{{"capsule":"a","resource":"elastos://mail/send","action":"launch","methods":["m"],{k}}}"#),
                StatusCode::BAD_REQUEST,
            ),
            (
                format!(r#"{{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":[],{k}}}"#),
                StatusCode::BAD_REQUEST,
            ),
            (
                format!(r#"{{"capsule":"a","resource":"elastos://*","action":"execute","methods":["m"],{k}}}"#),
                StatusCode::FORBIDDEN,
            ),
            (
                r#"{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":["m"],"agent_pubkey":"nothex"}"#.to_string(),
                StatusCode::BAD_REQUEST,
            ),
            // The web surface is narrower than the API: admin mints are refused server-side (P16),
            // even case-shifted to dodge a naive string check.
            (
                format!(r#"{{"capsule":"a","resource":"elastos://mail/send","action":"Admin","methods":["m"],{k}}}"#),
                StatusCode::FORBIDDEN,
            ),
            // G-M4 (Sprint 20): an UNBOUND mandate (no agent key) is refused from the web surface —
            // unbound is CLI-only. Server-side, not just the form.
            (
                r#"{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":["m"]}"#.to_string(),
                StatusCode::BAD_REQUEST,
            ),
            (
                r#"{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":["m"],"agent_pubkey":""}"#.to_string(),
                StatusCode::BAD_REQUEST,
            ),
        ];
        for (body, expected) in &cases {
            let resp = post_json(
                mandate_router(state.clone()),
                "/api/apps/mandates/standing-grants/issue",
                Some(token_hdr.clone()),
                body,
            )
            .await;
            assert_eq!(&resp.status(), expected, "guard for body {body}");
        }
        assert!(
            state.standing_service.list().is_empty(),
            "every refused mint must mint NOTHING"
        );
    }

    /// The agent-state view is fail-closed: no home-launch token ⇒ 401, no state leaks.
    #[tokio::test]
    async fn agent_state_route_requires_home_launch_token() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let denied = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/agent-state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    /// With a valid token the operator sees ALL agents' durable state (spanning principals), as the
    /// store holds it — the input_hash COMMITMENT, never content.
    #[tokio::test]
    async fn agent_state_route_reflects_the_store_across_principals() {
        let dir = tempfile::tempdir().unwrap();
        // Two different agents write state directly into the store the route reads.
        crate::agent_store::put_agent_state(dir.path(), "vm-a", "cursor", "cafe01", "g1", "i1").unwrap();
        crate::agent_store::put_agent_state(dir.path(), "vm-b", "flag", "beef02", "g2", "i2").unwrap();
        let app = mandate_router(state_for(dir.path()));
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/agent-state")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 2, "operator sees BOTH agents' state");
        let a = entries.iter().find(|e| e["capsule"] == "vm-a").unwrap();
        assert_eq!(a["key"], "cursor");
        assert_eq!(a["value_hash"], "cafe01", "the commitment, verbatim from the store");
        assert_eq!(a["grant_id"], "g1", "attributed to the mandate");
        assert!(entries.iter().any(|e| e["capsule"] == "vm-b" && e["key"] == "flag"));
    }

    /// The list route is fail-closed: no home-launch token ⇒ 401, no data leaks.
    #[tokio::test]
    async fn list_route_requires_home_launch_token() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let denied = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/standing-grants")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    /// With a valid mandates home token, the list reads the SAME registry the manager issues
    /// into — an issued mandate shows up, live.
    #[tokio::test]
    async fn list_route_reflects_the_shared_registry() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        // Issue a mandate straight into the shared registry (the manager mints, the service elevates).
        let token = state.capability_manager.grant(
            "vm-agent",
            ResourceId::new("elastos://mail/send".to_string()),
            Action::Execute,
            TokenConstraints::default(),
            Some(elastos_common::SecureTimestamp::after_secs(3600)),
        );
        let mut methods = std::collections::BTreeSet::new();
        methods.insert("send".to_string());
        let grant_id = state
            .standing_service
            .issue_from_token(&token, methods, None, None)
            .unwrap();

        let app = mandate_router(state);
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/standing-grants")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let mandates = json["mandates"].as_array().expect("mandates array");
        let card = mandates
            .iter()
            .find(|m| m["token_id"] == grant_id)
            .expect("the issued mandate is listed");
        assert_eq!(card["active"], true, "a just-issued mandate renders live");
        assert_eq!(card["revoked"], false);
        assert_eq!(card["capsule"], "vm-agent");
    }

    /// A mandate killed ONLY by a key-rotation / `revoke_all` EPOCH advance (no individual token
    /// revoke, no envelope flag) must NOT render "Live" on this liveness-proof surface. This is the
    /// guardian+red-team fold-in: the `active` bit now consults `is_epoch_valid`, shared with the API
    /// server via `mandate_cards`, so the two surfaces cannot drift.
    #[tokio::test]
    async fn list_route_marks_epoch_killed_mandate_dead() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let token = state.capability_manager.grant(
            "vm-agent",
            ResourceId::new("elastos://mail/send".to_string()),
            Action::Execute,
            TokenConstraints::default(),
            None,
        );
        let mut methods = std::collections::BTreeSet::new();
        methods.insert("send".to_string());
        let grant_id = state
            .standing_service
            .issue_from_token(&token, methods, None, None)
            .unwrap();
        // Kill the whole epoch WITHOUT individually revoking the token or touching the envelope.
        state.capability_manager.revoke_all("key rotation");
        // Sanity: the individual-revocation path is NOT what killed it — only the epoch advanced.
        assert!(
            !state
                .capability_manager
                .is_token_revoked(&TokenId::from_hex(&grant_id).unwrap())
                .await,
            "revoke_all must not individually revoke — this test isolates the epoch path"
        );

        let app = mandate_router(state);
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/standing-grants")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let card = json["mandates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["token_id"] == grant_id)
            .expect("the mandate is still listed");
        assert_eq!(card["active"], false, "an epoch-killed mandate never renders Live");
        assert_eq!(card["revoked"], true, "and it reads as revoked");
    }

    /// The receipt route is fail-closed too: no token ⇒ 401.
    #[tokio::test]
    async fn receipt_route_requires_home_launch_token() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let denied = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/mandate/deadbeef/receipt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    /// The KILL SWITCH is gated like the reads: no home-launch token ⇒ 401 AND the mandate stays
    /// live — an unauthenticated caller cannot kill (or probe) anything.
    #[tokio::test]
    async fn revoke_route_requires_home_launch_token_and_kills_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let token = state.capability_manager.grant(
            "vm-agent",
            ResourceId::new("elastos://mail/send".to_string()),
            Action::Execute,
            TokenConstraints::default(),
            None,
        );
        let mut methods = std::collections::BTreeSet::new();
        methods.insert("send".to_string());
        let grant_id = state
            .standing_service
            .issue_from_token(&token, methods, None, None)
            .unwrap();

        let app = mandate_router(state.clone());
        let denied = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/mandates/standing-grants/revoke")
                    .header("content-type", "application/json")
                    .body(Body::from(format!("{{\"grant_id\":\"{grant_id}\"}}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert!(
            state.standing_service.is_active(&grant_id),
            "an unauthenticated revoke must not kill the mandate"
        );
    }

    /// The in-shell kill switch: revoke over the route kills the mandate (it reads dead in the
    /// shared registry AND on the list), and a second pull reads `revoked:false` — idempotent,
    /// honestly reported.
    #[tokio::test]
    async fn revoke_route_kills_the_mandate_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let token = state.capability_manager.grant(
            "vm-agent",
            ResourceId::new("elastos://mail/send".to_string()),
            Action::Execute,
            TokenConstraints::default(),
            None,
        );
        let mut methods = std::collections::BTreeSet::new();
        methods.insert("send".to_string());
        let grant_id = state
            .standing_service
            .issue_from_token(&token, methods, None, None)
            .unwrap();
        assert!(state.standing_service.is_active(&grant_id));

        let app = mandate_router(state.clone());
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let revoke = |hdr: String, gid: String| {
            let app = app.clone();
            async move {
                let resp = app
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/api/apps/mandates/standing-grants/revoke")
                            .header(HOME_TOKEN_HEADER, hdr)
                            .header("content-type", "application/json")
                            .body(Body::from(format!("{{\"grant_id\":\"{gid}\"}}")))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }
        };

        // First pull: kills a live mandate.
        let first = revoke(token_hdr.clone(), grant_id.clone()).await;
        assert_eq!(first["revoked"], true, "a live mandate is killed by this call");
        assert!(
            !state.standing_service.is_active(&grant_id),
            "the envelope is dead in the SHARED registry"
        );

        // The list surface agrees — never renders the killed mandate Live.
        let cards = crate::api::handlers::capability::mandate_cards(
            &state.standing_service,
            &state.capability_manager,
        )
        .await;
        let card = cards
            .mandates
            .iter()
            .find(|c| c.token_id == grant_id)
            .expect("still listed (an operator surface shows what was killed)");
        assert!(!card.active, "killed mandate never renders Live");
        assert!(card.revoked);

        // Second pull: idempotent, honestly `false` (nothing live was killed).
        let second = revoke(token_hdr, grant_id).await;
        assert_eq!(second["revoked"], false, "double-revoke reads false");
    }

    /// A malformed grant id is rejected 400 BEFORE any kill path runs (fail-closed canonicalization).
    #[tokio::test]
    async fn revoke_route_rejects_malformed_id() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/mandates/standing-grants/revoke")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"grant_id":"not-hex"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// The Money panel routes are fail-closed twice over (Sprint 31): no home-launch token ⇒ 401
    /// (nothing leaks, nothing provisions); with a token but NO wired rail ⇒ 503 (absence
    /// reported, never an empty fabrication).
    #[tokio::test]
    async fn money_routes_are_gated_and_honest_about_an_unwired_rail() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path()); // pay_rail: None
        let app = mandate_router(state);
        // No token ⇒ 401 on all three.
        for (method, uri, body) in [
            ("GET", "/api/apps/mandates/money", String::new()),
            ("POST", "/api/apps/mandates/spend-budget", r#"{"capsule":"vm-a","limit":5}"#.into()),
            (
                "POST",
                "/api/apps/mandates/payments/reconcile",
                r#"{"idempotency_key":"flint-x","charged":false}"#.into(),
            ),
        ] {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
        // Token but unwired rail ⇒ 503.
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/money")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// End-to-end over the SHELL surface: provision a durable cap, see it (with pending
    /// visibility) on the money view, and resolve an indeterminate payment not-charged — the
    /// refund lands on the SAME meter the pay gate enforces with, exactly once.
    #[tokio::test]
    async fn money_panel_provisions_and_reconciles_over_the_shared_rail() {
        let dir = tempfile::tempdir().unwrap();
        let (capability_manager, standing_service) = manager_and_service();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
            )
            .unwrap(),
        );
        let ledger = Arc::new(
            crate::payment_ledger::PaymentLedger::open_durable(
                dir.path().join("payment_ledger.json"),
            )
            .unwrap(),
        );
        let state = MandateApiState {
            standing_service,
            capability_manager,
            data_dir: dir.path().to_path_buf(),
            pay_rail: Some(crate::api::server::PayRail {
                meter: meter.clone(),
                provider: Arc::new(crate::intent_executor::MockPaymentProvider::default()),
                ledger: ledger.clone(),
            }),
        };
        let app = mandate_router(state);
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();

        // Provision via the shell surface — the shared core applies + attests.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(token_hdr.clone()),
            r#"{"capsule":"vm-ap-agent","limit":500}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(meter.remaining("vm-ap-agent"), 500, "the ENFORCING meter holds the cap");

        // An indeterminate payment holds 200 and files a pending obligation (as the pay path
        // would): reserve on the meter + record on the ledger.
        meter.try_debit("vm-ap-agent", 200).unwrap();
        assert!(ledger.record(
            "flint-abc",
            "vm-ap-agent",
            "acme-vendor",
            200,
            crate::payment_ledger::PaymentStatus::Pending,
            "timeout"
        ));

        // The money view projects both: the budget with pending visibility + the work list.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/money")
                    .header(HOME_TOKEN_HEADER, token_hdr.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let budget = &json["budgets"][0];
        assert_eq!(budget["capsule"], "vm-ap-agent");
        assert_eq!(budget["remaining"], 300);
        assert_eq!(budget["pending_units"], 200, "held-unconfirmed is distinct");
        assert_eq!(json["pending"][0]["idempotency_key"], "flint-abc");

        // Reconcile not-charged over the shell surface: refund exactly once, 409 on retry.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/payments/reconcile",
            Some(token_hdr.clone()),
            r#"{"idempotency_key":"flint-abc","charged":false}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let out: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(out["refunded"], true);
        assert_eq!(meter.remaining("vm-ap-agent"), 500, "the refund landed on the shared meter");
        let retry = post_json(
            app,
            "/api/apps/mandates/payments/reconcile",
            Some(token_hdr),
            r#"{"idempotency_key":"flint-abc","charged":false}"#,
        )
        .await;
        assert_eq!(retry.status(), StatusCode::CONFLICT, "resolves exactly once");
        assert_eq!(meter.remaining("vm-ap-agent"), 500, "no double refund");
    }

    /// Honest absence: a well-formed token with no durable records ⇒ 404, never a fabricated receipt.
    #[tokio::test]
    async fn receipt_route_reports_absence_as_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let token_hdr = super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        // A syntactically valid (16-byte / 32-hex) token id that was never issued.
        let unknown = "0".repeat(32);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/apps/mandates/mandate/{unknown}/receipt"))
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
