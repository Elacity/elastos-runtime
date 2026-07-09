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
//! must be the launched capsule. The GET routes (list, agent-state, receipt, money, marketplace) are strictly read-only. The surface carries
//! FOUR mutations (Sprint 31 count — this enumeration is the surface's security story and must
//! stay literally true, council S31 G-F2):
//!
//! - REVOKE — the kill switch, fail-safe: only ever REMOVES authority (P11/P16).
//! - ISSUE (Sprint 15) — mints a mandate. Authority-GRANTING, so the trust argument is explicit:
//!   the home-launch token is minted by the runtime's own signing key when the SHELL launches the
//!   app, and the shell is already the runtime's grant root (G-M3 — the shell can issue any
//!   mandate anyway). This surface deliberately NARROWS itself below the raw API, server-side
//!   (no admin mints, bound-key required), because the residual threat is an XSS in the frame
//!   holding the in-URL token.
//! - SPEND-BUDGET (Sprint 31) — provisions a MONEY cap. Same G-M3 tier argument as issue, and the
//!   SAME narrowing discipline: the web surface enforces a CAP CEILING server-side
//!   ([`WEB_MAX_SPEND_CAP`], overridable via `ELASTOS_WEB_MAX_SPEND_CAP`) — an XSS in the frame
//!   cannot provision an unbounded cap; larger caps are a deliberate CLI/consent-broker act.
//! - PAYMENTS/RECONCILE (Sprint 31) — asserts the rail's verdict on ONE indeterminate payment.
//!   Same tier: a false "not charged" is a shell-root-level act (the shell can already raise any
//!   cap), single-shot by construction (the ledger resolves exactly once), chain-attested, and
//!   bounded per entry by the entry's own amount. No web-own narrowing beyond the shared core's
//!   guards — the verdict is exactly as dangerous as the cap raise it substitutes for, and
//!   ceiling-style bounds don't apply to a boolean.
//!
//! SPRINT 33 (folds council S31 red-team F1) — the money-authorization perimeter:
//!
//! 1. **The launch token is no longer URL-borne.** The shell launch response delivers it via an
//!    HttpOnly, SameSite=Strict Set-Cookie path-scoped to this sub-router's API prefix
//!    ([`MANDATES_SESSION_COOKIE`]); the launch URL carries only a non-secret `shell=1` marker.
//!    Frame script can no longer READ the credential (an XSS can still ride it same-origin —
//!    that is what the server-side narrowings above bound — but can never exfiltrate it), and it
//!    no longer leaks through history/logs/copy-paste. The gate still accepts the explicit
//!    header (tests, tools, pre-S33 clients). Cookie-authorized WRITES must also carry
//!    [`MANDATES_APP_MARKER_HEADER`] — a cookie is ambient browser state, and a cross-site page
//!    cannot set a custom header without a CORS preflight the gateway never grants to a
//!    foreign origin (the mandates routes emit no ACAO — pinned by the S31 regression test).
//! 2. **Money writes require a FRESH passkey verification, spent on use.** SPEND-BUDGET and
//!    RECONCILE additionally demand a proof-bound Home token minted by a WebAuthn ceremony at
//!    most [`MONEY_FRESH_WINDOW_SECS`] ago for the same principal (the wallet-send gate), and
//!    each verification authorizes exactly ONE money write — single-use consumption; replaying
//!    it on the same or the other money verb is refused. A leaked standing token or a ridden
//!    session can no longer move money. The honest claim (P12) is exactly "THIS operator's
//!    authenticator freshly approved ONE money write", nothing more. ISSUE and REVOKE keep the
//!    S15/S13 posture deliberately: revoke only ever REMOVES authority and must stay
//!    low-friction (it is the kill switch); issue's blast radius is bounded by the mint
//!    narrowings; extending fresh-binding to issue is a tracked follow-on (KNOWN_GAPS G-M9),
//!    not an oversight.
//!
//! RESIDUAL (KNOWN_GAPS G-M9): the spent-token guard is in-memory (a restart inside the ~3-min
//! freshness window could admit one replay), and the operator/local no-passkey posture cannot
//! make WEB money writes at all — the CLI/consent-broker API is that posture's money path
//! (fail closed; the panel says so).
//!
//! ISSUE delegates to the SAME shared mint path with every fail-closed guard intact (action
//! whitelist, non-empty methods, AUD-5 overbroad-wildcard refusal, agent-key validation,
//! durable-before-visible issuance); the money verbs delegate to the shared
//! `set_spend_budget_core` / `reconcile_payment_core` for the same no-drift reason.
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
    /// Sprint 33: fresh passkey tokens already SPENT on a money write, keyed by token digest with
    /// a prune-by deadline. One fresh verification authorizes exactly ONE money write on this
    /// surface — a second write with the same token is a replay and is refused. In-memory and
    /// bounded (entries outlive the freshness window by only a skew margin; see
    /// [`consume_fresh_money_token`]). RESIDUAL (documented in KNOWN_GAPS G-M9): a gateway
    /// restart clears it, so a token younger than the freshness window could be replayed once
    /// across a restart — the window is [`MONEY_FRESH_WINDOW_SECS`].
    pub(crate) spent_fresh_money_tokens:
        Arc<std::sync::Mutex<std::collections::HashMap<String, u64>>>,
    /// Sprint 38: the Marketplace panel's per-asset quote cache (TTL-bounded; see
    /// [`MARKET_QUOTE_TTL_SECS`]) — a browser refresh storm costs at most one chain read per
    /// asset per window.
    pub(crate) marketplace_quote_cache: MarketQuoteCache,
}

/// The mandates capsule id — must match `capsules/mandates/capsule.json`'s `name`.
pub(crate) const MANDATES_CAPSULE_ID: &str = "mandates";

/// Anti-CSRF marker for COOKIE-authorized writes (Sprint 33). A cookie is an ambient credential
/// — the browser attaches it to any same-site request regardless of who initiated it — so a
/// write authorized by the cookie must ALSO carry this custom header: setting a custom header
/// cross-origin forces a CORS preflight the gateway never grants to a foreign origin (no ACAO
/// on these routes — see the S31 F2 regression
/// test). Header-token writes don't need it — the token header itself is the unforgeable marker.
pub(crate) const MANDATES_APP_MARKER_HEADER: &str = "x-elastos-mandates-app";

/// How fresh the money-write passkey verification must be, in seconds — the same window the
/// wallet's send path uses (one shared operator expectation: "verify, then act, within ~3
/// minutes").
const MONEY_FRESH_WINDOW_SECS: u64 = 180;

/// Hard cap on the spent-token guard map (defense in depth; each entry requires a REAL passkey
/// ceremony to create, so an honest operator never approaches it). When full after pruning, money
/// writes are REFUSED — fail closed, never fail open by dropping replay protection.
const MONEY_SPENT_GUARD_MAX: usize = 4096;

/// The WEB surface's spend-cap ceiling (council S31 G-F1/RT-F5): the shell app can provision caps
/// up to this many units; anything larger is a deliberate CLI/consent-broker act. Server-side —
/// an XSS in the frame holding the in-URL token cannot provision an unbounded cap. Overridable
/// per deployment via `ELASTOS_WEB_MAX_SPEND_CAP` (the unit is the deployment's spend unit).
const WEB_MAX_SPEND_CAP_DEFAULT: u64 = 1_000_000;

fn web_max_spend_cap() -> u64 {
    match std::env::var("ELASTOS_WEB_MAX_SPEND_CAP") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(cap) => cap,
            // A typo'd override on a money ceiling must not fall back in silence.
            Err(e) => {
                tracing::warn!(
                    "ELASTOS_WEB_MAX_SPEND_CAP {raw:?} is not a u64 ({e}) — using the default \
                     ceiling {WEB_MAX_SPEND_CAP_DEFAULT}"
                );
                WEB_MAX_SPEND_CAP_DEFAULT
            }
        },
        Err(_) => WEB_MAX_SPEND_CAP_DEFAULT,
    }
}

/// The one launch-token gate every mandates route runs (Sprint 33): accepts the token from the
/// `x-elastos-home-token` header (tests, tools, pre-S33 clients) OR from the HttpOnly
/// path-scoped [`MANDATES_SESSION_COOKIE`] the shell launch response set. For WRITES authorized
/// by the COOKIE, the request must also carry [`MANDATES_APP_MARKER_HEADER`] — a cookie is
/// ambient browser state, and the custom header is what proves a same-origin script (not a
/// cross-site form) built the request. Returns the token's authority context for the money
/// routes' fresh-passkey check.
fn require_mandates_surface(
    state: &MandateApiState,
    headers: &HeaderMap,
    write: bool,
) -> Result<super::gateway_home_token::HomeLaunchTokenContext, Box<axum::response::Response>> {
    let (context, transport) = super::require_home_launch_token_context_transport(
        &state.data_dir,
        headers,
        MANDATES_CAPSULE_ID,
        MANDATES_SESSION_COOKIE,
    )
    .map_err(|err| Box::new(mandate_auth_error(err)))?;
    if write
        && transport == super::gateway_home_token::HomeTokenTransport::Cookie
        && !headers.contains_key(MANDATES_APP_MARKER_HEADER)
    {
        return Err(Box::new(
            (
                StatusCode::FORBIDDEN,
                format!(
                    "cookie-authorized writes must carry the {MANDATES_APP_MARKER_HEADER} header \
                     (cross-site request refused)"
                ),
            )
                .into_response(),
        ));
    }
    Ok(context)
}

/// The money-write authority gate (Sprint 33, council S31 F1): on top of the launch-token gate,
/// a money WRITE requires a FRESH passkey verification — a proof-bound Home token minted by a
/// WebAuthn ceremony at most [`MONEY_FRESH_WINDOW_SECS`] ago for the SAME principal the standing
/// token names — and each such verification is SPENT on first use (one ceremony = one write;
/// replaying the token on any money verb, same or different, is refused). What this proves is
/// exactly and only: THIS operator's authenticator freshly approved ONE state-changing money
/// operation. It does not scope WHICH write at mint time — single-use consumption is the
/// mechanism that keeps one assertion from authorizing a second write.
///
/// The standing 12h launch token alone can no longer move money: before this sprint the exact
/// failure was that a leaked launch URL (or a ridden session) could provision budgets and force
/// reconciliations for 12 hours.
fn require_fresh_money_authorization(
    state: &MandateApiState,
    headers: &HeaderMap,
    fresh_passkey_token: &str,
) -> Result<SpentFreshVerification, Box<axum::response::Response>> {
    let context = require_mandates_surface(state, headers, true)?;
    let canonical_assertion = super::require_fresh_passkey_home_token(
        &state.data_dir,
        fresh_passkey_token,
        &context,
        MONEY_FRESH_WINDOW_SECS,
    )
    .map_err(|err| {
        Box::new(
            (
                StatusCode::UNAUTHORIZED,
                format!(
                    "money writes require a fresh passkey verification (at most \
                     {MONEY_FRESH_WINDOW_SECS}s old): {err}"
                ),
            )
                .into_response(),
        )
    })?;
    consume_fresh_money_token(state, &canonical_assertion)
}

/// A CONSUMED fresh verification: proof the single-use guard admitted this write, carrying the
/// canonical key so the handler can RE-CREDIT it if the write is refused provably before any
/// money effect (council S33 red-team F1 — see [`SpentFreshVerification::refund`]).
struct SpentFreshVerification {
    key: String,
}

impl SpentFreshVerification {
    /// Re-credit the verification: the write it admitted was refused PROVABLY BEFORE any money
    /// effect — the surface's own guards (rail unwired, cap ceiling) or the shared cores' 4xx
    /// rejections (validation / unknown key / already-resolved, all pre-effect by the cores'
    /// construction) — so the operator's ceremony is not burned on a no-op. Same contract as
    /// the carrier's `DidNotAct` refunds (BUG-4): refund ONLY when a replay is a guaranteed
    /// no-op because nothing acted; any 5xx from the cores stays SPENT — it may have acted
    /// (indeterminate keeps the consumption, exactly like the pay path keeps its reservation).
    fn refund(self, state: &MandateApiState) {
        let mut spent = state
            .spent_fresh_money_tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        spent.remove(&self.key);
    }
}

/// Spend the fresh verification: exactly ONE money write per WebAuthn ceremony. Keyed by the
/// SHA-256 of the CANONICAL payload the signature was verified over — NEVER the raw token
/// string (council S33 guardian F1): the same assertion can be re-encoded into byte-different
/// token strings that all still verify, so a raw-string key would see each re-encoding as
/// unspent; the canonical form collapses every valid re-encoding onto one key, and altering any
/// field to mint a new key breaks the signature. Each real ceremony mints unique grant/session
/// ids, so distinct ceremonies never collide. Entries are pruned once they out-age the
/// freshness window (plus the same 60s issue-skew the verifier tolerates), so the map stays
/// bounded by the number of REAL ceremonies inside a ~4-minute window; if it is somehow full,
/// we refuse — fail closed, never drop replay protection to accept a write.
fn consume_fresh_money_token(
    state: &MandateApiState,
    canonical_assertion: &str,
) -> Result<SpentFreshVerification, Box<axum::response::Response>> {
    use sha2::Digest as _;
    let key = hex::encode(sha2::Sha256::digest(canonical_assertion.as_bytes()));
    let now = now_ts();
    let mut spent = state
        .spent_fresh_money_tokens
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    spent.retain(|_, prune_at| *prune_at > now);
    if spent.contains_key(&key) {
        return Err(Box::new(
            (
                StatusCode::FORBIDDEN,
                "this passkey verification was already spent on a money write — each money \
                 write needs its own fresh verification"
                    .to_string(),
            )
                .into_response(),
        ));
    }
    if spent.len() >= MONEY_SPENT_GUARD_MAX {
        return Err(Box::new(
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "money-write replay guard is at capacity; retry shortly".to_string(),
            )
                .into_response(),
        ));
    }
    spent.insert(key.clone(), now + MONEY_FRESH_WINDOW_SECS + 60);
    Ok(SpentFreshVerification { key })
}

/// A money WRITE body: the shared input plus the fresh passkey verification that authorizes this
/// one write. The inner shape is byte-identical to the consent-broker API's — the fresh token is
/// this WEB surface's own requirement (same narrowing discipline as the cap ceiling), so it
/// wraps rather than forks the shared struct.
#[derive(Debug, serde::Deserialize)]
struct FreshMoneyWrite<T> {
    /// A proof-bound Home token from `/api/auth/passkey/authenticate/complete`, at most
    /// [`MONEY_FRESH_WINDOW_SECS`] old, spent by this call.
    fresh_passkey_token: String,
    input: T,
}

/// GET /api/apps/mandates/standing-grants — the shell app's live mandate list.
///
/// Delegates to [`crate::api::handlers::capability::mandate_cards`] — the ONE shared projection the
/// API server also serves — so the liveness invariant (a revoked/expired/epoch-killed mandate never
/// renders "Live") can never drift between the two surfaces.
async fn mandates_list(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = require_mandates_surface(&state, &headers, false) {
        return *resp;
    }
    Json(
        crate::api::handlers::capability::mandate_cards(
            &state.standing_service,
            &state.capability_manager,
        )
        .await,
    )
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
    if let Err(resp) = require_mandates_surface(&state, &headers, false) {
        return *resp;
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
    if let Err(resp) = require_mandates_surface(&state, &headers, false) {
        return *resp;
    }
    let token_id = match TokenId::from_hex(token_id.trim()) {
        Ok(id) => id.to_string(),
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid token id: {e}")).into_response()
        }
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
    if let Err(resp) = require_mandates_surface(&state, &headers, true) {
        return *resp;
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
    if let Err(resp) = require_mandates_surface(&state, &headers, true) {
        return *resp;
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
    // Sprint 32 narrowing (same discipline as the agent-key requirement above): a mandate granted
    // from the shell must NAME its responsible entity — the operator is right there, and the
    // liability binding is the point of the web mint. Server-side, so an XSS in the frame cannot
    // mint an accountability-less mandate. Unrecorded-entity mints remain a deliberate CLI act.
    if body
        .responsible_entity
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            "a mandate granted from the shell must name its responsible entity — the operator/legal \
             entity DID accountable for the agent's acts (e.g. did:web:acme.example); \
             entity-less mandates are CLI-only"
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
/// reconciliation work list, and the poisoned flag. OPERATOR-scoped like the agent-state view: it
/// spans every capsule because the caller is the shell (the runtime's grant root, gated by the
/// same home-launch token), the trust level that already sees — and can raise — every cap — a READ-ONLY projection of the same meter and
/// ledger the pay gate enforces with (delegating to the shared
/// [`money_overview`](crate::api::handlers::capability::money_overview), so the two surfaces can
/// never drift). 503 when pay is unwired — absence reported, never an empty fabrication.
async fn money_view(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = require_mandates_surface(&state, &headers, false) {
        return *resp;
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
/// shell app. Authority-granting like `issue` (home-launch token = the shell grant root, G-M3)
/// AND, as a MONEY write (Sprint 33), requiring a fresh single-use passkey verification in the
/// body (`fresh_passkey_token` — see [`require_fresh_money_authorization`]). Delegates to the
/// ONE shared provisioning path
/// ([`set_spend_budget_core`](crate::api::handlers::capability::set_spend_budget_core)) so every
/// fail-closed guard (durable-only, slug bound, apply-then-attest with true rollback, loud double
/// failure) holds identically on both surfaces.
async fn money_set_budget(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
    Json(body): Json<FreshMoneyWrite<crate::api::handlers::capability::SetSpendBudgetInput>>,
) -> axum::response::Response {
    let spent = match require_fresh_money_authorization(&state, &headers, &body.fresh_passkey_token)
    {
        Ok(spent) => spent,
        Err(resp) => return *resp,
    };
    let body = body.input;
    let Some(rail) = &state.pay_rail else {
        // Pre-effect refusal: nothing is wired, nothing acted — the ceremony is re-credited.
        spent.refund(&state);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired — a spend budget nothing enforces is refused".to_string(),
        )
            .into_response();
    };
    // The web surface's own narrowing (module doc; mirrors the issue route's admin refusal):
    // enforced HERE, server-side, so an XSS in the frame is still ceiling-bound. The raw
    // consent-broker API carries no ceiling — a larger cap is a deliberate operator act.
    let ceiling = web_max_spend_cap();
    if body.limit > ceiling {
        // Pre-effect refusal: the core never ran — the ceremony is re-credited.
        spent.refund(&state);
        return (
            StatusCode::FORBIDDEN,
            format!(
                "caps above {ceiling} units are set from the CLI/consent-broker API, not the \
                 shell app (ELASTOS_WEB_MAX_SPEND_CAP raises this surface's ceiling)"
            ),
        )
            .into_response();
    }
    match crate::api::handlers::capability::set_spend_budget_core(
        &rail.meter,
        Some(&rail.ledger),
        state.capability_manager.audit_log(),
        body,
    ) {
        Ok(out) => Json(out).into_response(),
        Err((status, msg)) => {
            // A 4xx from the shared core is a pre-effect rejection by construction (validation,
            // slug bound) — re-credit. A 5xx may have partially acted (apply-then-attest) — the
            // ceremony stays spent, mirroring the pay path's indeterminate-keeps-reservation.
            if status.is_client_error() {
                spent.refund(&state);
            }
            (status, msg).into_response()
        }
    }
}

/// POST /api/apps/mandates/payments/reconcile — resolve ONE indeterminate payment against the
/// rail's verdict, from the shell app. A MONEY write (Sprint 33): requires its own fresh
/// single-use passkey verification in the body (`fresh_passkey_token`), like `spend-budget` —
/// one ceremony authorizes one write. Delegates to the ONE shared reconciliation path
/// ([`reconcile_payment_core`](crate::api::handlers::capability::reconcile_payment_core)):
/// exactly-once resolve, refund only on durable Ok, structured 404/409/503, chain attestation —
/// identical on both surfaces by construction.
async fn money_reconcile(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
    Json(body): Json<FreshMoneyWrite<crate::api::handlers::capability::ReconcilePaymentInput>>,
) -> axum::response::Response {
    let spent = match require_fresh_money_authorization(&state, &headers, &body.fresh_passkey_token)
    {
        Ok(spent) => spent,
        Err(resp) => return *resp,
    };
    let body = body.input;
    let Some(rail) = &state.pay_rail else {
        // Pre-effect refusal: nothing is wired, nothing acted — the ceremony is re-credited.
        spent.refund(&state);
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
        Err((status, msg)) => {
            // 404 (unknown key) / 409 (already resolved) are pre-effect lookups by the core's
            // construction — re-credit. A 5xx may be indeterminate — the ceremony stays spent.
            if status.is_client_error() {
                spent.refund(&state);
            }
            (status, msg).into_response()
        }
    }
}

// ─────────────────────────── The Marketplace panel (Sprint 38) ───────────────────────────
//
// STRICTLY READ-ONLY: every pixel is a projection of the one enforcing registry + meter + ledger
// (same Arcs as the pay gate, by construction). The panel never gains a "buy" verb — operators
// GRANT (the issue route), agents ACT (the signed-intent dispatch routes on the API server);
// this surface only shows what the mandates scope and what the ledger says happened.

pub(crate) use crate::market_quote::{
    claim_or_serve, quote_outcome, CachedQuote, MarketQuote, MarketQuoteCache,
    MARKET_QUOTE_TTL_SECS,
};

/// At most this many FRESH chain reads per view. Cache hits are free (they cost no chain read and
/// never consume a slot), so with the TTL this rotates quote coverage across refreshes instead of
/// permanently starving the alphabetically-last assets: view 1 reads assets 1-8, view 2 finds
/// them cached and reads 9-16, and so on.
const MARKET_MAX_QUOTED_ASSETS: usize = 8;
/// How many SETTLED (terminal) ledger entries the buys table projects. Pending buys are NEVER
/// truncated — an operator watch surface must not let a flood of new terminals push a live
/// obligation out of sight.
const MARKET_BUYS_LIMIT: usize = 64;

#[derive(serde::Serialize)]
struct MarketAssetView {
    /// The DRM asset reference (the pay resource's payee suffix — the KID / content id).
    asset: String,
    /// Token ids of the ACTIVE pay-mandates scoped to this asset.
    mandates: Vec<String>,
    /// The live on-chain terms (cached up to [`MARKET_QUOTE_TTL_SECS`]); absent iff unquoted.
    #[serde(skip_serializing_if = "Option::is_none")]
    quote: Option<MarketQuote>,
    /// True when this asset had no cached quote AND this view's fresh-read slots were already
    /// taken — stated, not silently dropped; the TTL rotation reaches it on a later refresh.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unquoted_over_cap: bool,
}

#[derive(serde::Serialize)]
struct MarketBuyView {
    /// The payee — the DRM asset reference the buy targeted.
    asset: String,
    /// The acting capsule (agent identity).
    capsule: String,
    /// Spend units authorized by the signed intent.
    amount: u64,
    /// Machine state: refused | pending | charged | confirmed | refunded.
    state: String,
    /// The honest operator wording for `state` (a broadcast is NEVER "purchased").
    detail: String,
    /// The broadcast tx hash, when the rail reference carries one (`drm:tx=…`).
    #[serde(skip_serializing_if = "Option::is_none")]
    tx: Option<String>,
    /// The mandate the buy acted under, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    mandate: Option<String>,
}

#[derive(serde::Serialize)]
struct MarketplaceView {
    /// The rights mode the quotes were read under — "chain" is live truth; "dev"/"chainmock"
    /// quote FREE (price 0), and the panel must say so rather than display a fake price.
    rights_mode: String,
    assets: Vec<MarketAssetView>,
    buys: Vec<MarketBuyView>,
    /// How many entries the ledger holds in total — when it exceeds `buys.len()`, the table is
    /// honestly a WINDOW (pending always included; only the terminal tail is capped).
    buys_total: usize,
    buys_limit: usize,
    quote_ttl_secs: u64,
}

/// Map one ledger record to its honest display row. The wording rule (P12): a broadcast-accepted
/// buy reads "awaiting chain confirmation" — `confirmed` is reserved for the chain's own verdict
/// (or the operator's explicit reconcile), never the broadcast.
fn market_buy_view(record: &crate::payment_ledger::PaymentRecord) -> MarketBuyView {
    use crate::payment_ledger::PaymentStatus;
    // Display-bounded: the rail note is rail-controlled bytes — a real tx hash is 66 chars, so
    // an over-long "tx" is garbage that must not bloat the payload/DOM.
    let tx = crate::drm_marketplace::parse_drm_tx(&record.rail_note)
        .map(|t| t.chars().take(80).collect::<String>());
    let (state, detail) = match record.status {
        PaymentStatus::Pending => (
            "pending",
            if tx.is_some() {
                "broadcast — awaiting chain confirmation"
            } else {
                "indeterminate — awaiting reconciliation"
            },
        ),
        PaymentStatus::Performed => ("charged", "charged (rail-attested)"),
        PaymentStatus::ResolvedCharged => (
            "confirmed",
            if tx.is_some() {
                "confirmed on-chain"
            } else {
                "confirmed (reconciled against the rail)"
            },
        ),
        PaymentStatus::NotCharged => ("refused", "refused — nothing charged"),
        PaymentStatus::ResolvedNotCharged => ("refunded", "refunded — the reservation came back"),
    };
    MarketBuyView {
        asset: record.payee.clone(),
        capsule: record.capsule.clone(),
        amount: record.amount,
        state: state.to_string(),
        detail: detail.to_string(),
        tx,
        mandate: record.token_id.clone(),
    }
}

/// GET /api/apps/mandates/marketplace — the Marketplace panel's one read: the assets the ACTIVE
/// pay-mandates scope (with live, cached, fan-out-bounded quotes) and the recent buys as the
/// ledger records them. 503 without a wired rail (a marketplace with no money path is not a
/// marketplace — same posture as the Money panel).
async fn marketplace_view(
    State(state): State<MandateApiState>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Err(resp) = require_mandates_surface(&state, &headers, false) {
        return *resp;
    }
    let Some(rail) = &state.pay_rail else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no payment rail is wired".to_string(),
        )
            .into_response();
    };

    // 1. The assets: every ACTIVE mandate whose envelope authorizes runtime.pay on a pay
    //    resource, grouped by asset (deterministic order — BTreeMap).
    let cards = crate::api::handlers::capability::mandate_cards(
        &state.standing_service,
        &state.capability_manager,
    )
    .await;
    let mut by_asset: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for card in &cards.mandates {
        if !card.active || !card.methods.iter().any(|m| m == "runtime.pay") {
            continue;
        }
        let Some(asset) = card
            .resource
            .strip_prefix(crate::intent_executor::PAY_PREFIX)
        else {
            continue;
        };
        if asset.is_empty() {
            continue;
        }
        by_asset
            .entry(asset.to_string())
            .or_default()
            .push(card.token_id.clone());
    }

    // 2. The quotes. ONE claim pass under the cache lock decides everything (single-flight,
    //    council S38 fold): a fresh cached quote is served free (a cache hit never consumes a
    //    fresh-read slot); a fresh IN-FLIGHT sentinel means another view is already reading —
    //    serve "in progress", never a duplicate read; a miss CLAIMS the slot (sentinel inserted
    //    under the lock, before any read starts) up to MARKET_MAX_QUOTED_ASSETS fresh reads per
    //    view. So "at most one live chain read per asset per TTL window" is literal under any
    //    concurrency, and coverage ROTATES across refreshes instead of starving the tail.
    let now = now_ts();
    crate::market_quote::prune(&state.marketplace_quote_cache, now); // once per view
    let mut assets = Vec::with_capacity(by_asset.len());
    let mut to_quote: Vec<String> = Vec::new();
    for (asset, mandates) in by_asset {
        // The SHARED single-flight claim pass (crate::market_quote — the same spine the
        // runtime.market_quote affordance uses): serve fresh free, respect an in-flight claim,
        // claim up to this view's fresh-read budget, and state the over-budget tail.
        let may_claim = to_quote.len() < MARKET_MAX_QUOTED_ASSETS;
        let (quote, over_cap) =
            match claim_or_serve(&state.marketplace_quote_cache, &asset, now, may_claim) {
                CachedQuote::Fresh(q) => (Some(q), false),
                CachedQuote::InFlight => (None, false),
                CachedQuote::Claimed => {
                    to_quote.push(asset.clone());
                    (None, false)
                }
                CachedQuote::NotClaimed => (None, true),
            };
        assets.push(MarketAssetView {
            asset,
            mandates,
            quote,
            unquoted_over_cap: over_cap,
        });
    }
    if !to_quote.is_empty() {
        let fresh = tokio::task::spawn_blocking(move || {
            to_quote
                .into_iter()
                .map(|asset| {
                    let quote = quote_outcome(crate::api::buy_authority::quote_buy(
                        &asset,
                        &crate::api::buy_authority::BuyTarget::default(),
                    ));
                    (asset, quote)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        for (asset, quote) in fresh {
            // Stamped AFTER the read returned (council S38 red-team F4): a slow fetch must not
            // be served as "fresh" for a full TTL past its actual read time.
            crate::market_quote::fill(
                &state.marketplace_quote_cache,
                &asset,
                quote.clone(),
                now_ts(),
            );
            if let Some(view) = assets.iter_mut().find(|a| a.asset == asset) {
                view.quote = Some(quote);
            }
        }
    }

    // 3. The buys: every PENDING entry (a live obligation is never pushed out of sight by a
    //    flood of newer terminals — council S38 fold) plus the most recent settled tail, newest
    //    first, with the window stated via buys_total/buys_limit.
    let ledger = &rail.ledger;
    let buys_total = ledger.len();
    let mut records = ledger.pending();
    for r in ledger.recent(MARKET_BUYS_LIMIT) {
        if r.status != crate::payment_ledger::PaymentStatus::Pending {
            records.push(r);
        }
    }
    records.sort_by_key(|r| std::cmp::Reverse(r.seq));
    let buys: Vec<MarketBuyView> = records.iter().map(market_buy_view).collect();

    Json(MarketplaceView {
        rights_mode: format!("{:?}", crate::api::rights_authority::rights_mode()).to_lowercase(),
        assets,
        buys,
        buys_total,
        buys_limit: MARKET_BUYS_LIMIT,
        quote_ttl_secs: MARKET_QUOTE_TTL_SECS,
    })
    .into_response()
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
        // The Marketplace panel (Sprint 38): strictly read-only — assets scoped by pay-mandates
        // with live (cached, fan-out-bounded) quotes, and the buys as the ledger records them.
        .route("/api/apps/mandates/marketplace", get(marketplace_view))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use elastos_runtime::capability::token::TokenConstraints;
    use elastos_runtime::capability::{Action, CapabilityStore, ResourceId, StandingGrantService};
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
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        }
    }

    /// Sprint 33 fixture: a standing mandates launch token plus `fresh` DISTINCT fresh-passkey
    /// Home tokens for the SAME principal. Each fresh token is backed by its own active
    /// proof-bound session grant with unique session/grant ids — exactly what each real WebAuthn
    /// ceremony mints in production (so no two fixtures collapse to the same token string).
    fn money_write_tokens(dir: &std::path::Path, fresh: usize) -> (String, Vec<String>) {
        let principal = "person:local:money-op".to_string();
        let standing_ctx = HomeLaunchTokenContext {
            principal_id: principal.clone(),
            session_id: "local:standing".to_string(),
            proof_binding_id: None,
            grant_id: "grant:standing".to_string(),
        };
        let standing = super::super::issue_home_launch_token_with_context(
            dir,
            MANDATES_CAPSULE_ID,
            &standing_ctx,
        )
        .unwrap();
        let now = now_ts();
        let mut tokens = Vec::new();
        for i in 0..fresh {
            let ctx = HomeLaunchTokenContext {
                principal_id: principal.clone(),
                session_id: format!("auth:fresh-{i}"),
                proof_binding_id: Some(format!("proof:passkey:money-{i}")),
                grant_id: format!("grant:fresh-{i}"),
            };
            crate::auth::store_session_grant(
                dir,
                elastos_runtime::auth::AuthSessionGrantV1 {
                    schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
                    grant_id: ctx.grant_id.clone(),
                    session_id: ctx.session_id.clone(),
                    principal_id: ctx.principal_id.clone(),
                    proof_binding_id: ctx.proof_binding_id.clone().unwrap(),
                    issued_at: now,
                    expires_at: now + 3600,
                    apps: vec![HOME_CAPSULE_ID.to_string()],
                },
            )
            .unwrap();
            tokens.push(
                super::super::issue_home_launch_token_with_context(dir, HOME_CAPSULE_ID, &ctx)
                    .unwrap(),
            );
        }
        (standing, tokens)
    }

    /// A money-write body in the Sprint 33 shape: the shared input wrapped with the fresh
    /// verification that authorizes this one write.
    fn money_body(fresh_token: &str, input: &str) -> String {
        format!(
            r#"{{"fresh_passkey_token":{},"input":{input}}}"#,
            serde_json::json!(fresh_token)
        )
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
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
                r#"{{"capsule":"vm-agent","resource":"elastos://mail/send","action":"execute","methods":["send"],"ttl_secs":3600,"agent_pubkey":"{agent}","responsible_entity":"did:web:acme.example"}}"#
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let out: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let grant_id = out["grant_id"].as_str().expect("grant id returned");
        assert_eq!(
            out["token_id"], grant_id,
            "grant id IS the backing token id"
        );
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
        assert!(
            card.agent_bound,
            "the card shows the mandate is agent-bound"
        );
        assert_eq!(card.agent_pubkey.as_deref(), Some(agent.as_str()));
        // Sprint 32: the responsible-entity liability binding round-trips onto the card.
        assert_eq!(
            card.responsible_entity.as_deref(),
            Some("did:web:acme.example"),
            "the card surfaces WHO is accountable"
        );
    }

    /// The gateway mint enforces the SAME fail-closed guards as the API server (shared helper):
    /// unknown action 400, empty methods 400, AUD-5 bare scheme wildcard 403, malformed agent key
    /// 400 — and NONE of them mint anything.
    #[tokio::test]
    async fn issue_route_enforces_the_shared_guards() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for(dir.path());
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        // A valid 64-hex agent key so downstream guards (wildcard, key-format) are the ones tested,
        // not the new G-M4 bound-required check (which fires first when the key is absent).
        let k = "\"agent_pubkey\":\"".to_string()
            + &"ab".repeat(32)
            + "\",\"responsible_entity\":\"did:web:acme.example\"";
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
                r#"{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":["m"],"agent_pubkey":"nothex","responsible_entity":"did:web:acme.example"}"#.to_string(),
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
            // Sprint 32: a bound mandate with NO responsible entity is refused from the shell
            // (entity-less mandates are CLI-only) — server-side, an XSS in the frame can't skip it.
            (
                format!(r#"{{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":["m"],"agent_pubkey":"{}"}}"#, "ab".repeat(32)),
                StatusCode::BAD_REQUEST,
            ),
            // A malformed responsible entity (not a did: URI) fails closed at the shared core.
            (
                format!(r#"{{"capsule":"a","resource":"elastos://mail/send","action":"execute","methods":["m"],"agent_pubkey":"{}","responsible_entity":"acme corp"}}"#, "ab".repeat(32)),
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
        crate::agent_store::put_agent_state(dir.path(), "vm-a", "cursor", "cafe01", "g1", "i1")
            .unwrap();
        crate::agent_store::put_agent_state(dir.path(), "vm-b", "flag", "beef02", "g2", "i2")
            .unwrap();
        let app = mandate_router(state_for(dir.path()));
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["entries"].as_array().expect("entries array");
        assert_eq!(entries.len(), 2, "operator sees BOTH agents' state");
        let a = entries.iter().find(|e| e["capsule"] == "vm-a").unwrap();
        assert_eq!(a["key"], "cursor");
        assert_eq!(
            a["value_hash"], "cafe01",
            "the commitment, verbatim from the store"
        );
        assert_eq!(a["grant_id"], "g1", "attributed to the mandate");
        assert!(entries
            .iter()
            .any(|e| e["capsule"] == "vm-b" && e["key"] == "flag"));
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
            .issue_from_token(&token, methods, None, None, None)
            .unwrap();

        let app = mandate_router(state);
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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
            .issue_from_token(&token, methods, None, None, None)
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
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let card = json["mandates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["token_id"] == grant_id)
            .expect("the mandate is still listed");
        assert_eq!(
            card["active"], false,
            "an epoch-killed mandate never renders Live"
        );
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
            .issue_from_token(&token, methods, None, None, None)
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
            .issue_from_token(&token, methods, None, None, None)
            .unwrap();
        assert!(state.standing_service.is_active(&grant_id));

        let app = mandate_router(state.clone());
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
                let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }
        };

        // First pull: kills a live mandate.
        let first = revoke(token_hdr.clone(), grant_id.clone()).await;
        assert_eq!(
            first["revoked"], true,
            "a live mandate is killed by this call"
        );
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
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
            (
                "POST",
                "/api/apps/mandates/spend-budget",
                r#"{"fresh_passkey_token":"x","input":{"capsule":"vm-a","limit":5}}"#.into(),
            ),
            (
                "POST",
                "/api/apps/mandates/payments/reconcile",
                r#"{"fresh_passkey_token":"x","input":{"idempotency_key":"flint-x","charged":false}}"#.into(),
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
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
                drm_confirmer: None,
                quote_cache: Arc::default(),
            }),
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        };
        let app = mandate_router(state);
        // Sprint 33: money writes each need their OWN fresh passkey verification.
        let (token_hdr, fresh) = money_write_tokens(dir.path(), 3);

        // Provision via the shell surface — the shared core applies + attests.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(token_hdr.clone()),
            &money_body(&fresh[0], r#"{"capsule":"vm-ap-agent","limit":500}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            500,
            "the ENFORCING meter holds the cap"
        );

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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
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
            &money_body(
                &fresh[1],
                r#"{"idempotency_key":"flint-abc","charged":false}"#,
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let out: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(out["refunded"], true);
        assert_eq!(
            meter.remaining("vm-ap-agent"),
            500,
            "the refund landed on the shared meter"
        );
        let retry = post_json(
            app,
            "/api/apps/mandates/payments/reconcile",
            Some(token_hdr),
            &money_body(
                &fresh[2],
                r#"{"idempotency_key":"flint-abc","charged":false}"#,
            ),
        )
        .await;
        assert_eq!(
            retry.status(),
            StatusCode::CONFLICT,
            "resolves exactly once"
        );
        assert_eq!(meter.remaining("vm-ap-agent"), 500, "no double refund");
    }

    /// Council S31 G-F1: the WEB surface's cap ceiling — a provision above it is refused 403
    /// server-side (an XSS in the frame cannot provision an unbounded cap) and provisions NOTHING;
    /// at-or-below the ceiling still works. The raw consent-broker API carries no ceiling.
    #[tokio::test]
    async fn web_spend_budget_is_ceiling_bound_server_side() {
        let dir = tempfile::tempdir().unwrap();
        let (capability_manager, standing_service) = manager_and_service();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
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
                ledger: Arc::new(crate::payment_ledger::PaymentLedger::new()),
                drm_confirmer: None,
                quote_cache: Arc::default(),
            }),
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        };
        let app = mandate_router(state);
        let (token_hdr, fresh) = money_write_tokens(dir.path(), 2);
        let over = WEB_MAX_SPEND_CAP_DEFAULT + 1;
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(token_hdr.clone()),
            &money_body(
                &fresh[0],
                &format!(r#"{{"capsule":"vm-xss","limit":{over}}}"#),
            ),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "above the ceiling is refused"
        );
        assert_eq!(
            meter.snapshot("vm-xss"),
            None,
            "the refused provision set NOTHING"
        );
        let resp = post_json(
            app,
            "/api/apps/mandates/spend-budget",
            Some(token_hdr),
            &money_body(
                &fresh[1],
                &format!(r#"{{"capsule":"vm-ok","limit":{WEB_MAX_SPEND_CAP_DEFAULT}}}"#),
            ),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "at the ceiling still provisions"
        );
    }

    /// Council S31 red-team F2 regression: the gateway money routes must emit NO
    /// Access-Control-Allow-Origin for a foreign Origin — a future refactor adding permissive
    /// CORS here would hand cross-origin pages the preflight pass the CSRF analysis relies on
    /// being absent.
    #[tokio::test]
    async fn money_routes_emit_no_cors_allowance_for_foreign_origins() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/apps/mandates/payments/reconcile")
                    .header("origin", "https://evil.example")
                    .header("access-control-request-method", "POST")
                    .header("access-control-request-headers", "x-elastos-home-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().get("access-control-allow-origin").is_none(),
            "no ACAO for a foreign origin — the preflight must fail"
        );
    }

    /// Sprint 33 ratchet (a): the standing 12h launch token ALONE can no longer move money.
    /// Before this sprint the exact failure was that a leaked launch URL could provision budgets
    /// for its whole 12h life; now a money write without a fresh passkey verification — empty,
    /// or a stale/unbound token — is refused and provisions NOTHING.
    #[tokio::test]
    async fn money_write_with_the_standing_token_alone_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (capability_manager, standing_service) = manager_and_service();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
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
                ledger: Arc::new(crate::payment_ledger::PaymentLedger::new()),
                drm_confirmer: None,
                quote_cache: Arc::default(),
            }),
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        };
        let app = mandate_router(state);
        let (standing, _) = money_write_tokens(dir.path(), 0);

        // No fresh verification at all.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body("", r#"{"capsule":"vm-a","limit":5}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "empty fresh token refused"
        );

        // The standing token ITSELF is not a fresh verification (it is not proof-bound) — a
        // session-rider replaying the credential they already hold gains nothing.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body(&standing, r#"{"capsule":"vm-a","limit":5}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unbound token refused"
        );
        assert_eq!(meter.snapshot("vm-a"), None, "nothing was provisioned");

        // Reads stay on the standing session alone — no regression, no passkey friction.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/money")
                    .header(HOME_TOKEN_HEADER, standing)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "reads need no fresh verification"
        );
    }

    /// Sprint 33 ratchets (b)+(c): one fresh passkey verification authorizes exactly ONE money
    /// write. Replaying it on the SAME verb is refused (b), and spending it on set-budget then
    /// presenting it to reconcile is refused too (c) — single-use consumption subsumes cross-verb
    /// binding: the assertion cannot authorize a second write of ANY kind.
    #[tokio::test]
    async fn fresh_passkey_verification_is_single_use_across_money_verbs() {
        let dir = tempfile::tempdir().unwrap();
        let (capability_manager, standing_service) = manager_and_service();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
            )
            .unwrap(),
        );
        let ledger = Arc::new(crate::payment_ledger::PaymentLedger::new());
        let state = MandateApiState {
            standing_service,
            capability_manager,
            data_dir: dir.path().to_path_buf(),
            pay_rail: Some(crate::api::server::PayRail {
                meter: meter.clone(),
                provider: Arc::new(crate::intent_executor::MockPaymentProvider::default()),
                ledger: ledger.clone(),
                drm_confirmer: None,
                quote_cache: Arc::default(),
            }),
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        };
        let app = mandate_router(state);
        let (standing, fresh) = money_write_tokens(dir.path(), 1);

        // First write spends the verification.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body(&fresh[0], r#"{"capsule":"vm-once","limit":50}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the first write is authorized"
        );
        assert_eq!(meter.remaining("vm-once"), 50);

        // (b) Replay on the SAME verb: refused, and the cap is NOT re-set.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body(&fresh[0], r#"{"capsule":"vm-once","limit":999}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "replay on the same verb refused"
        );
        assert_eq!(
            meter.remaining("vm-once"),
            50,
            "the replayed write applied NOTHING"
        );

        // (c) Cross-verb: the spent verification cannot authorize reconcile either. The refusal
        // is at the authorization gate — before any ledger lookup.
        meter.try_debit("vm-once", 10).unwrap();
        assert!(ledger.record(
            "flint-xv",
            "vm-once",
            "vendor",
            10,
            crate::payment_ledger::PaymentStatus::Pending,
            "timeout"
        ));
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/payments/reconcile",
            Some(standing),
            &money_body(
                &fresh[0],
                r#"{"idempotency_key":"flint-xv","charged":false}"#,
            ),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "cross-verb replay refused"
        );
        assert_eq!(
            meter.remaining("vm-once"),
            40,
            "no refund was applied by the refused call"
        );
    }

    /// Sprint 33 ratchet (cookie transport): the HttpOnly path-scoped cookie authorizes the
    /// surface exactly like the header — reads work over it — but a cookie-authorized WRITE
    /// must carry the anti-CSRF marker header (a cookie is ambient; the custom header is what a
    /// cross-site page cannot send without a preflight this gateway refuses).
    #[tokio::test]
    async fn cookie_transport_reads_work_and_writes_demand_the_csrf_marker() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let (standing, _) = money_write_tokens(dir.path(), 0);
        let cookie = format!("{MANDATES_SESSION_COOKIE}={standing}");

        // Read over the cookie alone: authorized.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/standing-grants")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "cookie authorizes reads");

        // Cookie-authorized WRITE without the marker: refused as potential CSRF.
        let revoke_body = format!(r#"{{"grant_id":"{}"}}"#, "0".repeat(32));
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/mandates/standing-grants/revoke")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(revoke_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "a cookie-authorized write without the app marker is refused"
        );

        // Same write WITH the marker: passes the gate (the unknown mandate honestly reads
        // `revoked:false` — the kill switch stays reachable over the cookie).
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/mandates/standing-grants/revoke")
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .header(MANDATES_APP_MARKER_HEADER, "1")
                    .body(Body::from(revoke_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the marker admits the same-origin write"
        );
    }

    /// Council S33 guardian F1 (the fold's ship-blocker): the single-use guard keys on the
    /// CANONICAL assertion the signature covers, never the raw token string. A spent fresh token
    /// RE-ENCODED into a byte-different string (pretty-printed JSON, same envelope) still
    /// VERIFIES as the same assertion — and must still read as SPENT. Before the fix the guard
    /// hashed the raw string, so one WebAuthn ceremony re-encoded N ways authorized N money
    /// writes. The 403 (not 401) on the replay is the proof the re-encoding passed verification
    /// and the GUARD — not the signature check — is what refused it.
    #[tokio::test]
    async fn a_re_encoded_spent_verification_is_still_spent() {
        let dir = tempfile::tempdir().unwrap();
        let (capability_manager, standing_service) = manager_and_service();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
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
                ledger: Arc::new(crate::payment_ledger::PaymentLedger::new()),
                drm_confirmer: None,
                quote_cache: Arc::default(),
            }),
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        };
        let app = mandate_router(state);
        let (standing, fresh) = money_write_tokens(dir.path(), 1);

        // Spend the verification on a legitimate write.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body(&fresh[0], r#"{"capsule":"vm-malleate","limit":50}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(meter.remaining("vm-malleate"), 50);

        // Re-encode the SAME token into a byte-different string: decode → parse → pretty-print
        // → re-encode. The signature still verifies (it covers the canonical payload, which is
        // unchanged); only the transport bytes differ.
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&fresh[0])
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let pretty = serde_json::to_vec_pretty(&value).unwrap();
        assert_ne!(pretty, bytes, "the re-encoding must be byte-different");
        let re_encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&pretty);
        assert_ne!(re_encoded, fresh[0]);

        let resp = post_json(
            app,
            "/api/apps/mandates/spend-budget",
            Some(standing),
            &money_body(&re_encoded, r#"{"capsule":"vm-malleate","limit":999}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "the re-encoded replay must read SPENT (403), not unverified (401)"
        );
        assert_eq!(
            meter.remaining("vm-malleate"),
            50,
            "the replay applied NOTHING"
        );
    }

    /// Council S33 red-team F1: a refusal PROVABLY BEFORE any money effect (here the web
    /// ceiling) re-credits the ceremony — the operator's one verification still buys their one
    /// write — and the verification is spent only when a write actually reaches the core.
    #[tokio::test]
    async fn a_pre_effect_refusal_re_credits_the_fresh_verification() {
        let dir = tempfile::tempdir().unwrap();
        let (capability_manager, standing_service) = manager_and_service();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                dir.path().join("spend_meter.json"),
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
                ledger: Arc::new(crate::payment_ledger::PaymentLedger::new()),
                drm_confirmer: None,
                quote_cache: Arc::default(),
            }),
            spent_fresh_money_tokens: Arc::default(),
            marketplace_quote_cache: Arc::default(),
        };
        let app = mandate_router(state);
        let (standing, fresh) = money_write_tokens(dir.path(), 1);
        let over = WEB_MAX_SPEND_CAP_DEFAULT + 1;

        // Refused over the ceiling — pre-effect, so the ceremony is re-credited.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body(
                &fresh[0],
                &format!(r#"{{"capsule":"vm-cred","limit":{over}}}"#),
            ),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(meter.snapshot("vm-cred"), None, "nothing was provisioned");

        // The SAME verification now authorizes the corrected write.
        let resp = post_json(
            app.clone(),
            "/api/apps/mandates/spend-budget",
            Some(standing.clone()),
            &money_body(&fresh[0], r#"{"capsule":"vm-cred","limit":100}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "the re-credited verification still works"
        );
        assert_eq!(meter.remaining("vm-cred"), 100);

        // …and having bought its one applied write, it is now spent for good.
        let resp = post_json(
            app,
            "/api/apps/mandates/spend-budget",
            Some(standing),
            &money_body(&fresh[0], r#"{"capsule":"vm-cred","limit":101}"#),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "one applied write, then spent"
        );
        assert_eq!(meter.remaining("vm-cred"), 100);
    }

    /// Honest absence: a well-formed token with no durable records ⇒ 404, never a fabricated receipt.
    #[tokio::test]
    async fn receipt_route_reports_absence_as_404() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
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
    // ────────────────────────── Marketplace panel (Sprint 38) ──────────────────────────

    /// The marketplace read is gated exactly like every other route: no launch token ⇒ 401.
    #[tokio::test]
    async fn marketplace_route_requires_home_launch_token() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let denied = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/marketplace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    /// No rail wired ⇒ 503, honestly unwired — same posture as the Money panel.
    #[tokio::test]
    async fn marketplace_without_a_rail_reads_unwired() {
        let dir = tempfile::tempdir().unwrap();
        let app = mandate_router(state_for(dir.path()));
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/marketplace")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// The panel is a pure PROJECTION: pay-mandates scope the asset list (non-pay mandates never
    /// appear), and the buys table renders the ledger's records with the HONEST wording — a
    /// broadcast-accepted buy reads "awaiting chain confirmation", never anything like
    /// "purchased"; a confirmed one carries its tx; a refusal reads refused.
    #[tokio::test]
    async fn marketplace_projects_pay_mandates_and_ledger_buys_with_honest_wording() {
        use crate::payment_ledger::{PaymentLedger, PaymentStatus};
        use elastos_runtime::primitives::spend::SpendMeter;

        let dir = tempfile::tempdir().unwrap();
        let mut state = state_for(dir.path());
        let meter = Arc::new(SpendMeter::new());
        let ledger = Arc::new(PaymentLedger::new());
        state.pay_rail = Some(crate::api::server::PayRail {
            meter,
            provider: Arc::new(crate::intent_executor::MockPaymentProvider::default()),
            ledger: ledger.clone(),
            drm_confirmer: None,
            quote_cache: Arc::default(),
        });

        // A pay-mandate for asset QmMovie…
        let pay_token = state.capability_manager.grant(
            "vm-shopper",
            ResourceId::new("elastos://runtime/pay/QmMovie".to_string()),
            Action::Execute,
            TokenConstraints::default(),
            None,
        );
        let mut pay_methods = std::collections::BTreeSet::new();
        pay_methods.insert("runtime.pay".to_string());
        let pay_grant = state
            .standing_service
            .issue_from_token(&pay_token, pay_methods, None, None, None)
            .unwrap();
        // …and a NON-pay mandate that must never surface as a marketplace asset.
        let mail_token = state.capability_manager.grant(
            "vm-agent",
            ResourceId::new("elastos://mail/send".to_string()),
            Action::Execute,
            TokenConstraints::default(),
            None,
        );
        let mut mail_methods = std::collections::BTreeSet::new();
        mail_methods.insert("send".to_string());
        state
            .standing_service
            .issue_from_token(&mail_token, mail_methods, None, None, None)
            .unwrap();

        // Ledger truth: a broadcast-pending DRM buy, a chain-confirmed one, and a refusal.
        assert!(ledger.record_with_token(
            "flint-pending",
            "vm-shopper",
            "QmMovie",
            5,
            PaymentStatus::Pending,
            "drm:tx=0xAB;op=0xop;tid=7",
            Some(&pay_grant),
        ));
        assert!(ledger.record_with_token(
            "flint-confirmed",
            "vm-shopper",
            "QmMovie",
            5,
            PaymentStatus::ResolvedCharged,
            "drm:tx=0xCD;op=0xop;tid=7",
            Some(&pay_grant),
        ));
        assert!(ledger.record(
            "flint-refused",
            "vm-shopper",
            "QmMovie",
            999,
            PaymentStatus::NotCharged,
            "refused by spend cap",
        ));

        // Pre-populate the quote cache so the projection is fully deterministic (no chain).
        state.marketplace_quote_cache.lock().unwrap().insert(
            "QmMovie".to_string(),
            crate::market_quote::MarketQuoteSlot {
                quoted_at: now_ts(),
                quote: Some(MarketQuote {
                    price: Some("5000000".to_string()),
                    pay_token: Some("0xUSDC".to_string()),
                    supply: Some(3),
                    error: None,
                }),
            },
        );

        let app = mandate_router(state);
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/marketplace")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Assets: exactly the pay-mandate's asset, quote served CACHE-FIRST (no chain read).
        let assets = json["assets"].as_array().unwrap();
        assert_eq!(
            assets.len(),
            1,
            "only pay-mandates scope marketplace assets"
        );
        assert_eq!(assets[0]["asset"], "QmMovie");
        assert_eq!(assets[0]["mandates"][0], pay_grant);
        assert_eq!(
            assets[0]["quote"]["price"], "5000000",
            "the quote is served from the TTL cache — a panel refresh is not a chain read"
        );

        // Buys: honest wording per state, tx extracted from the DRM rail note.
        let buys = json["buys"].as_array().unwrap();
        assert_eq!(buys.len(), 3);
        let by_state = |st: &str| {
            buys.iter()
                .find(|b| b["state"] == st)
                .unwrap_or_else(|| panic!("no {st} row"))
        };
        let pending = by_state("pending");
        assert_eq!(pending["detail"], "broadcast — awaiting chain confirmation");
        assert_eq!(pending["tx"], "0xAB");
        assert!(
            !pending["detail"].as_str().unwrap().contains("purchas"),
            "a broadcast is NEVER worded as a purchase"
        );
        let confirmed = by_state("confirmed");
        assert_eq!(confirmed["detail"], "confirmed on-chain");
        assert_eq!(confirmed["tx"], "0xCD");
        assert_eq!(confirmed["mandate"], pay_grant);
        let refused = by_state("refused");
        assert_eq!(refused["detail"], "refused — nothing charged");
        assert_eq!(refused["amount"], 999);
        assert!(
            refused.get("tx").is_none(),
            "no tx on a never-broadcast refusal"
        );
    }
    /// Council S38 fold (guardian F1 + red-team F1): quote coverage ROTATES — cache hits are
    /// free (they never consume a fresh-read slot), so with more assets than the per-view
    /// fresh-read cap, the first view reads the cap's worth and the SECOND view (finding those
    /// cached) reads the rest: no asset is permanently starved of a quote.
    #[tokio::test]
    async fn quote_coverage_rotates_instead_of_starving_the_tail() {
        use crate::payment_ledger::PaymentLedger;
        use elastos_runtime::primitives::spend::SpendMeter;

        let dir = tempfile::tempdir().unwrap();
        let mut state = state_for(dir.path());
        state.pay_rail = Some(crate::api::server::PayRail {
            meter: Arc::new(SpendMeter::new()),
            provider: Arc::new(crate::intent_executor::MockPaymentProvider::default()),
            ledger: Arc::new(PaymentLedger::new()),
            drm_confirmer: None,
            quote_cache: Arc::default(),
        });
        // MARKET_MAX_QUOTED_ASSETS + 2 assets, each behind an active pay-mandate.
        for i in 0..(MARKET_MAX_QUOTED_ASSETS + 2) {
            let token = state.capability_manager.grant(
                "vm-shopper",
                ResourceId::new(format!("elastos://runtime/pay/Qm{i:02}")),
                Action::Execute,
                TokenConstraints::default(),
                None,
            );
            let mut methods = std::collections::BTreeSet::new();
            methods.insert("runtime.pay".to_string());
            state
                .standing_service
                .issue_from_token(&token, methods, None, None, None)
                .unwrap();
        }

        let app = mandate_router(state.clone());
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let view = |app: Router, hdr: String| async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri("/api/apps/mandates/marketplace")
                        .header(HOME_TOKEN_HEADER, hdr)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        };

        // View 1: the cap's worth of assets get fresh reads (in CI the chain read fails fast, so
        // they land as bounded error quotes — still QUOTED outcomes); exactly 2 are over-cap.
        let first = view(app.clone(), token_hdr.clone()).await;
        let over_cap = |j: &serde_json::Value| {
            j["assets"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| a["unquoted_over_cap"] == true)
                .count()
        };
        assert_eq!(
            over_cap(&first),
            2,
            "the tail waits, stated — never silently dropped"
        );

        // View 2: the first batch is CACHED (free), so the tail gets the fresh-read slots.
        let second = view(app, token_hdr).await;
        assert_eq!(
            over_cap(&second),
            0,
            "cache hits are free — the rotation reaches every asset by the second view"
        );
        assert!(
            second["assets"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["quote"].is_object()),
            "every asset ends up with a quote outcome (live terms or a bounded error)"
        );
    }

    /// Council S38 fold (red-team F2): a live PENDING buy can never be pushed out of the buys
    /// table by a flood of newer settled entries — pending is always shown; only the settled
    /// tail is windowed, and the window is STATED via buys_total.
    #[tokio::test]
    async fn a_pending_buy_is_never_truncated_by_a_flood_of_settled_entries() {
        use crate::payment_ledger::{PaymentLedger, PaymentStatus};
        use elastos_runtime::primitives::spend::SpendMeter;

        let dir = tempfile::tempdir().unwrap();
        let mut state = state_for(dir.path());
        let ledger = Arc::new(PaymentLedger::new());
        state.pay_rail = Some(crate::api::server::PayRail {
            meter: Arc::new(SpendMeter::new()),
            provider: Arc::new(crate::intent_executor::MockPaymentProvider::default()),
            ledger: ledger.clone(),
            drm_confirmer: None,
            quote_cache: Arc::default(),
        });

        // The OLDEST entry is a live pending obligation…
        assert!(ledger.record_with_token(
            "flint-obligation",
            "vm-shopper",
            "QmMovie",
            5,
            PaymentStatus::Pending,
            "drm:tx=0xAB;op=0xop;tid=7",
            None,
        ));
        // …then a flood of newer settled entries, more than the window.
        for i in 0..(MARKET_BUYS_LIMIT + 10) {
            assert!(ledger.record(
                &format!("flint-noise-{i}"),
                "vm-flooder",
                "QmOther",
                1,
                PaymentStatus::NotCharged,
                "refused",
            ));
        }

        let app = mandate_router(state);
        let token_hdr =
            super::super::issue_home_launch_token(dir.path(), MANDATES_CAPSULE_ID).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/mandates/marketplace")
                    .header(HOME_TOKEN_HEADER, token_hdr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let buys = json["buys"].as_array().unwrap();
        assert!(
            buys.iter()
                .any(|b| b["state"] == "pending" && b["asset"] == "QmMovie"),
            "the live obligation is ALWAYS visible, however many settled entries arrive"
        );
        assert_eq!(
            json["buys_total"].as_u64().unwrap() as usize,
            MARKET_BUYS_LIMIT + 11,
            "the window is stated, never silent"
        );
        assert!(
            buys.len() <= MARKET_BUYS_LIMIT + 1,
            "the settled tail stays windowed (pending rides on top of the cap)"
        );
    }
}
