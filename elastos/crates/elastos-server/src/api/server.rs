//! HTTP server implementation
//!
//! Provides the session/capability HTTP surface used by the runtime, Home, and
//! browser-hosted capsule adapters.

use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;

use axum::{
    http::{HeaderName, HeaderValue, Request},
    middleware as axum_middleware,
    routing::{delete, get, head, post, put},
    Extension, Router,
};
use elastos_runtime::primitives::audit::AuditLog;
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::api::handlers::docs::DocsState;
use crate::api::handlers::identity::IdentityState;
use crate::api::handlers::{self, CapabilityState, NamespaceState};
use crate::api::middleware::{
    auth_middleware, consent_broker_only_middleware, rate_limit_middleware, ApiState,
    RateLimitState, RateLimiter,
};
use crate::api::routes;
use crate::runtime::Runtime;
use elastos_runtime::capability::evaluator::ShellPassthroughVerifier;
use elastos_runtime::capability::{
    AutoGrantVerifier, CapabilityManager, PendingRequestStore, PolicyEvaluator, RulesVerifier,
};
use elastos_runtime::namespace::NamespaceStore;
use elastos_runtime::provider::ProviderRegistry;
use elastos_runtime::session::SessionRegistry;

/// Middleware that sets Cross-Origin-Opener-Policy and Cross-Origin-Embedder-Policy headers.
/// Required for SharedArrayBuffer (used by threaded WASM like mgba-wasm).
async fn cross_origin_isolation(
    request: Request<axum::body::Body>,
    next: axum_middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-embedder-policy"),
        HeaderValue::from_static("require-corp"),
    );
    response
}

fn is_allowed_local_origin(origin: &HeaderValue) -> bool {
    let s = match origin.to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    match url::Url::parse(s) {
        Ok(url) => matches!(
            url.host_str(),
            Some("127.0.0.1") | Some("localhost") | Some("::1") | Some("[::1]")
        ),
        Err(_) => false,
    }
}

/// Bootstrap state for web capsules (provides token + manifest info to the frontend)
#[derive(Clone)]
pub struct CapsuleBootstrapState {
    pub token: String,
    pub manifest: elastos_common::CapsuleManifest,
}

/// Configuration for the full HTTP API server (Phase 5+).
pub struct ServerConfig {
    pub runtime: Arc<Runtime>,
    pub session_registry: Arc<SessionRegistry>,
    pub capability_manager: Arc<CapabilityManager>,
    /// The shared runtime mandate registry (so the Home gateway's Mandates app sees the SAME
    /// standing grants this API server issues). `None` ⇒ this server creates its own (isolated) —
    /// used by serve paths without a Home gateway.
    pub standing_service: Option<Arc<elastos_runtime::capability::intent::StandingGrantService>>,
    pub pending_store: Arc<PendingRequestStore>,
    pub namespace_store: Option<Arc<NamespaceStore>>,
    pub provider_registry: Option<Arc<ProviderRegistry>>,
    pub audit_log: Option<Arc<elastos_runtime::primitives::audit::AuditLog>>,
    pub identity_state: Option<IdentityState>,
    pub docs_dir: Option<PathBuf>,
    pub addr: String,
    pub capsule_dir: Option<PathBuf>,
    /// Directory containing data capsule files (served at /capsule-data/)
    pub data_dir: Option<PathBuf>,
    /// Bootstrap state for web capsule auto-configuration
    pub bootstrap_state: Option<CapsuleBootstrapState>,
    pub tls_config: Option<axum_server::tls_rustls::RustlsConfig>,
    /// Capsule supervisor for VM-based capsule lifecycle (supervisor path only)
    pub supervisor: Option<Arc<crate::supervisor::Supervisor>>,
    /// Readiness signal — sent after the TCP listener binds successfully.
    /// Replaces startup sleep heuristics with a deterministic handshake.
    pub ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Shared secret for the attach endpoint — callers prove local ownership by
    /// presenting this secret (read from the chmod-600 runtime-coords file) to
    /// mint short-lived session tokens.  When `None` the attach endpoint is disabled.
    pub attach_secret: Option<String>,
    /// Operator-approved host helpers that must live as long as this API server.
    pub host_helpers: Vec<HostHelperProcess>,
    /// The wired payment rail (ONE meter+provider+ledger trio for the whole process — see
    /// [`PayRail`]), built by [`build_pay_rail`] in the infrastructure layer so the gateway's
    /// Mandates-app Money panel shares the SAME Arcs. `None` ⇒ pay honestly unwired.
    pub pay_rail: Option<PayRail>,
}

/// The wired payment rail: ONE meter + provider + ledger trio shared by the executor's pay gate,
/// the API server's provisioning/reconciliation surface, and the gateway's Mandates-app Money
/// panel — the same-Arc rule that keeps enforcement and every projection from ever disagreeing.
/// Built once per process by [`build_pay_rail`] (the meter and ledger hold single-opener flocks,
/// so a second build in the same data_dir would refuse).
#[derive(Clone)]
pub struct PayRail {
    pub meter: Arc<elastos_runtime::primitives::spend::SpendMeter>,
    pub provider: Arc<dyn crate::intent_executor::PaymentProvider>,
    pub ledger: Arc<crate::payment_ledger::PaymentLedger>,
    /// The chain-confirmation reader, present ONLY on the DRM rail (Sprint 37) — what the
    /// in-runtime reconcile scheduler polls pending buys against. `None` on the HTTP/mock rails
    /// (their pendings are operator-reconciled), which also keeps the scheduler structurally
    /// un-armable there.
    pub drm_confirmer: Option<Arc<dyn crate::drm_marketplace::DrmConfirmer>>,
}

/// Rail selection, fail-closed (Sprint 29/31; see the serve() doc block for the full rules):
/// `ELASTOS_PAYMENT_ENDPOINT` (validated: https or loopback-http, well-formed) wires the REAL
/// `HttpPaymentProvider` and REQUIRES the durable meter+ledger; else `ELASTOS_ALLOW_MOCK_PAYMENTS`
/// wires the Mock (dev/demo; in-memory stores only in the bare no-data_dir shape); else `None` —
/// pay honestly unwired.
pub fn build_pay_rail(data_dir: Option<&std::path::Path>) -> Option<PayRail> {
    let real_endpoint = std::env::var("ELASTOS_PAYMENT_ENDPOINT").ok();
    let mock_allowed = std::env::var("ELASTOS_ALLOW_MOCK_PAYMENTS").is_ok();
    let open_durable_meter = || match data_dir {
        Some(dir) => match elastos_runtime::primitives::spend::SpendMeter::open_durable(
            dir.join("spend_meter.json"),
        ) {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                tracing::error!(
                    "spend meter snapshot could not be opened ({e}) — runtime.pay stays UNWIRED \
                     (fail-closed) rather than booting over a possibly-refilled or contended \
                     money cap"
                );
                None
            }
        },
        None => None,
    };
    let open_ledger = || match data_dir {
        Some(dir) => match crate::payment_ledger::PaymentLedger::open_durable(
            dir.join("payment_ledger.json"),
        ) {
            Ok(l) => Some(Arc::new(l)),
            Err(e) => {
                tracing::error!(
                    "payment ledger could not be opened ({e}) — runtime.pay stays UNWIRED (a \
                     lost pending set would orphan reconciliation obligations)"
                );
                None
            }
        },
        None => Some(Arc::new(crate::payment_ledger::PaymentLedger::new())),
    };
    // The DRM marketplace rail (Sprint 34): `ELASTOS_PAYMENT_RAIL=drm` settles `runtime.pay`
    // acts on the Elacity on-chain DRM marketplace instead of an HTTPS endpoint. Same spine — it
    // REQUIRES the durable meter+ledger (real money on non-durable stores is refused), shares the
    // exact two-generals classification and receipt path. The buyer principal/subject/ledger come
    // from env; the live chain is exercised only by the operator runbook, never CI.
    let drm_rail = std::env::var("ELASTOS_PAYMENT_RAIL")
        .map(|v| v.trim().eq_ignore_ascii_case("drm"))
        .unwrap_or(false);
    if drm_rail {
        if real_endpoint.is_some() {
            tracing::warn!(
                "both ELASTOS_PAYMENT_RAIL=drm and ELASTOS_PAYMENT_ENDPOINT are set — the DRM \
                 marketplace rail wins; the HTTP endpoint is ignored"
            );
        }
        // Mock-money discipline (council S34 guardian F4): the DRM rail settles through
        // `buy_access`, whose DEV rights-mode fabricates a synthetic tx hash with NO chain — that
        // is MOCK money, and the S29 rule is that mock money requires the explicit
        // `ELASTOS_ALLOW_MOCK_PAYMENTS` opt-in. So a non-Chain rights mode (Dev/ChainMock) wires
        // the DRM rail ONLY when mock payments are explicitly allowed; otherwise it stays UNWIRED,
        // fail-closed. Release builds default to Chain and refuse dev modes at boot, so production
        // is unaffected; this closes the dev-build asymmetry.
        let mode = super::rights_authority::rights_mode();
        if !matches!(mode, super::rights_authority::RightsMode::Chain) && !mock_allowed {
            tracing::error!(
                "ELASTOS_PAYMENT_RAIL=drm with a non-Chain rights mode ({mode:?}) fabricates \
                 synthetic settlements — that is MOCK money and requires \
                 ELASTOS_ALLOW_MOCK_PAYMENTS to be set explicitly; runtime.pay stays UNWIRED"
            );
            return None;
        }
        // The declared meter-unit⇄pay-token mapping (Sprint 36 — the price gate). One spend unit
        // authorizes this many pay-token SMALLEST units (e.g. USDC 6-decimals: `1000000` ⇒ 1
        // spend unit == 1 USDC). REQUIRED and >= 1: a Chain-mode DRM rail refuses to wire without
        // it, rather than silently assuming 1 spend unit == 1 wei — so the operator must DECLARE
        // the unit and the cap becomes a literal on-chain ceiling. Dev/ChainMock (mock-opt-in) may
        // omit it (a free quote makes the gate a no-op) ⇒ default 1.
        let is_chain_mode =
            matches!(mode, super::rights_authority::RightsMode::Chain);
        let spend_unit = match std::env::var("ELASTOS_DRM_SPEND_UNIT") {
            Ok(v) => match v.trim().parse::<u128>() {
                Ok(n) if n >= 1 => n,
                _ => {
                    tracing::error!(
                        "ELASTOS_DRM_SPEND_UNIT={v:?} is not a positive integer — the DRM price \
                         gate cannot map spend units to the pay token; runtime.pay stays UNWIRED"
                    );
                    return None;
                }
            },
            Err(_) if is_chain_mode => {
                tracing::error!(
                    "ELASTOS_PAYMENT_RAIL=drm on the live Chain rail requires \
                     ELASTOS_DRM_SPEND_UNIT (pay-token smallest-units per spend unit, e.g. \
                     1000000 for USDC) so the cap is a literal on-chain ceiling — refusing to \
                     wire the DRM rail with an undeclared unit mapping (fail-closed)"
                );
                return None;
            }
            Err(_) => 1, // dev/chain-mock: the quote is free, so the gate is a no-op
        };
        // The pay-token the unit mapping is FOR (council S36 F3). REQUIRED on the live Chain rail:
        // the `spend_unit` denominates one token, and listings can quote heterogeneous tokens, so a
        // buy in any other token is refused. Omitted in dev/chain-mock (free quote ⇒ no gate).
        let expected_pay_token = match std::env::var("ELASTOS_DRM_PAY_TOKEN") {
            Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
            _ if is_chain_mode => {
                tracing::error!(
                    "ELASTOS_PAYMENT_RAIL=drm on the live Chain rail requires ELASTOS_DRM_PAY_TOKEN \
                     (the pay-token address the spend-unit mapping denominates, e.g. the USDC \
                     address) so the cap is a literal ceiling in ONE token — refusing to wire the \
                     DRM rail without it (fail-closed)"
                );
                return None;
            }
            _ => None,
        };
        let principal = std::env::var("ELASTOS_DRM_BUYER_PRINCIPAL").unwrap_or_default();
        let subject = std::env::var("ELASTOS_DRM_BUYER_SUBJECT").unwrap_or_default();
        let ledger = std::env::var("ELASTOS_DDRM_LEDGER").unwrap_or_default();
        // Loud-at-boot on a misconfigured buyer (council S34 guardian F5): both are fail-closed at
        // act time (an empty buyer ⇒ "wallet not linked" ⇒ refund; an empty ledger ⇒ resolve
        // error ⇒ refund), but a silent misconfig should be visible at boot like the HTTP rail's
        // refuse-on-malformed-endpoint. Wallet-signing mode legitimately runs with an empty
        // subject (the managed account is authoritative), so subject is not warned.
        if principal.trim().is_empty() {
            tracing::warn!(
                "ELASTOS_PAYMENT_RAIL=drm wired with an EMPTY ELASTOS_DRM_BUYER_PRINCIPAL — every \
                 buy will fail closed (wallet not linked) until it is set"
            );
        }
        if ledger.trim().is_empty() {
            tracing::warn!(
                "ELASTOS_PAYMENT_RAIL=drm wired with an EMPTY ELASTOS_DDRM_LEDGER — every KID \
                 resolve will fail closed until it is set"
            );
        }
        return match (open_durable_meter(), open_ledger()) {
            (Some(meter), Some(payment_ledger)) => {
                tracing::info!(
                    "runtime.pay is wired to the DRM marketplace rail (durable spend meter; buys \
                     settle on-chain via buy_authority; provision caps at POST /api/spend-budgets)"
                );
                let marketplace = Arc::new(crate::drm_marketplace::ChainDrmMarketplace::new(
                    principal, subject, ledger,
                ));
                Some(PayRail {
                    meter,
                    provider: Arc::new(crate::drm_marketplace::DrmMarketplaceProvider::new(
                        marketplace.clone(),
                        marketplace.clone(),
                        spend_unit,
                        expected_pay_token,
                    )),
                    ledger: payment_ledger,
                    drm_confirmer: Some(marketplace),
                })
            }
            _ => {
                tracing::error!(
                    "ELASTOS_PAYMENT_RAIL=drm is set but the DURABLE spend meter/ledger is \
                     unavailable — real money on non-durable stores is refused; runtime.pay \
                     stays UNWIRED"
                );
                None
            }
        };
    }
    if let Some(endpoint) = real_endpoint {
        if mock_allowed {
            tracing::warn!(
                "both ELASTOS_PAYMENT_ENDPOINT and ELASTOS_ALLOW_MOCK_PAYMENTS are set — the \
                 REAL rail wins; the mock is ignored"
            );
        }
        // Endpoint validation, fail-closed (council S29 red-team F1 / guardian F3-F4): a money
        // order + bearer token must never transit plaintext, and a malformed value must refuse at
        // BOOT (a builder error at pay time would strand reservations). https is REQUIRED;
        // plaintext http is allowed ONLY to loopback (a same-box sidecar adapter — inside the
        // host trust boundary).
        let endpoint_ok = match url::Url::parse(&endpoint) {
            Ok(u) => match u.scheme() {
                "https" => true,
                "http" => {
                    let loopback = matches!(
                        u.host(),
                        Some(url::Host::Ipv4(ip)) if ip.is_loopback()
                    ) || matches!(
                        u.host(),
                        Some(url::Host::Ipv6(ip)) if ip.is_loopback()
                    ) || matches!(u.host_str(), Some("localhost"));
                    if !loopback {
                        tracing::error!(
                            "ELASTOS_PAYMENT_ENDPOINT is plaintext http to a non-loopback host — \
                             payment orders and the bearer token would transit cleartext (MITM \
                             could forge Performed receipts); runtime.pay stays UNWIRED. Use https."
                        );
                    }
                    loopback
                }
                other => {
                    tracing::error!(
                        "ELASTOS_PAYMENT_ENDPOINT has unsupported scheme {other:?} — runtime.pay \
                         stays UNWIRED"
                    );
                    false
                }
            },
            Err(e) => {
                tracing::error!(
                    "ELASTOS_PAYMENT_ENDPOINT is not a valid URL ({e}) — runtime.pay stays UNWIRED"
                );
                false
            }
        };
        let token = std::env::var("ELASTOS_PAYMENT_TOKEN").ok();
        if endpoint_ok && token.is_none() {
            tracing::warn!(
                "the REAL payment rail is wired WITHOUT a bearer token (ELASTOS_PAYMENT_TOKEN \
                 unset) — the endpoint must authenticate callers some other way"
            );
        }
        return match (endpoint_ok, open_durable_meter(), open_ledger()) {
            (true, Some(meter), Some(ledger)) => {
                tracing::info!(
                    "runtime.pay is wired to the REAL payment rail at {endpoint} (durable spend \
                     meter; provision caps at POST /api/spend-budgets)"
                );
                Some(PayRail {
                    meter,
                    provider: Arc::new(crate::intent_executor::HttpPaymentProvider::new(
                        endpoint, token,
                    )),
                    ledger,
                    drm_confirmer: None,
                })
            }
            (true, _, _) => {
                tracing::error!(
                    "ELASTOS_PAYMENT_ENDPOINT is set but the DURABLE spend meter/ledger is \
                     unavailable — real money on non-durable stores is refused; runtime.pay \
                     stays UNWIRED"
                );
                None
            }
            (false, _, _) => None,
        };
    }
    if mock_allowed {
        // With a data_dir, the durable stores are used (an unopenable snapshot leaves pay
        // UNWIRED — never a silent fall-through to fresh in-memory caps); only the bare
        // no-data_dir test/embedded shape gets in-memory stores.
        let stores = match data_dir {
            Some(_) => open_durable_meter().zip(open_ledger()),
            None => {
                // No in-repo binary reaches this branch (every serve path builds the rail from
                // the infrastructure layer's real data_dir — council S31 G-F3); it exists for
                // embedders calling build_pay_rail(None) directly.
                tracing::warn!(
                    "no data_dir — runtime.pay gets an IN-MEMORY spend meter (test/embedded \
                     only); the provisioning surface will refuse money caps on it"
                );
                Some((
                    Arc::new(elastos_runtime::primitives::spend::SpendMeter::new()),
                    Arc::new(crate::payment_ledger::PaymentLedger::new()),
                ))
            }
        };
        return stores.map(|(meter, ledger)| {
            tracing::warn!(
                "runtime.pay is wired to the MOCK payment rail (ELASTOS_ALLOW_MOCK_PAYMENTS) — \
                 receipts attest SIMULATED payments; DEV/DEMO ONLY, never production"
            );
            PayRail {
                meter,
                provider: Arc::new(crate::intent_executor::MockPaymentProvider::default()),
                ledger,
                drm_confirmer: None,
            }
        });
    }
    None
}

/// The DRM confirmation scheduler's env-derived arming decision (Sprint 37 — closes KNOWN_GAPS
/// MKT-DRM 2b). Extracted PURE-over-env so CI pins the fail-closed arming rules without a tokio
/// runtime. Returns `Some((interval, per-tick batch cap))` iff the scheduler should run:
///
/// - OFF BY DEFAULT (P16 — no ambient background chain poller): arms ONLY when
///   `ELASTOS_DRM_RECONCILE_INTERVAL_SECS` is set (u64 ≥ 1) AND the wired rail carries a DRM
///   confirmer. An interval on a non-DRM rail warns and stays off (its pendings are
///   operator-reconciled; there is no chain to poll).
/// - FAIL-CLOSED on a malformed value: an unparseable interval or batch REFUSES to arm with an
///   error log — a scheduler must never guess its own cadence or bound.
/// - `ELASTOS_DRM_RECONCILE_BATCH` (usize ≥ 1, default 64) bounds one tick's work; the overflow
///   is counted (`DrmReconcileSummary::skipped`) and picked up next tick, oldest-first.
pub fn drm_reconcile_schedule_from_env(
    rail_has_confirmer: bool,
) -> Option<(std::time::Duration, usize)> {
    let interval_raw = std::env::var("ELASTOS_DRM_RECONCILE_INTERVAL_SECS").ok()?;
    let interval_secs = match interval_raw.trim().parse::<u64>() {
        Ok(n) if n >= 1 => n,
        _ => {
            tracing::error!(
                "ELASTOS_DRM_RECONCILE_INTERVAL_SECS={interval_raw:?} is not a positive integer \
                 — the DRM confirmation scheduler stays OFF (fail-closed, no guessed cadence)"
            );
            return None;
        }
    };
    if !rail_has_confirmer {
        tracing::warn!(
            "ELASTOS_DRM_RECONCILE_INTERVAL_SECS is set but the wired payment rail is not the \
             DRM rail — the confirmation scheduler stays OFF (nothing on-chain to poll; HTTP/mock \
             pendings are operator-reconciled)"
        );
        return None;
    }
    let batch = match std::env::var("ELASTOS_DRM_RECONCILE_BATCH") {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => {
                tracing::error!(
                    "ELASTOS_DRM_RECONCILE_BATCH={raw:?} is not a positive integer — the DRM \
                     confirmation scheduler stays OFF (fail-closed, no guessed bound)"
                );
                return None;
            }
        },
        Err(_) => DRM_RECONCILE_BATCH_DEFAULT,
    };
    Some((std::time::Duration::from_secs(interval_secs), batch))
}

/// Default per-tick cap on DRM pendings processed by the scheduler.
const DRM_RECONCILE_BATCH_DEFAULT: usize = 64;

/// Spawn the in-runtime DRM confirmation scheduler (Sprint 37): a thin `tokio` interval loop over
/// [`crate::drm_marketplace::drm_reconcile_tick`] — the SAME reconciliation the manual
/// `POST /api/payments/reconcile` path drives, so the scheduler adds ZERO new money-moving code.
/// Ticks run on the blocking pool (the confirmer shells out to the chain-provider), so rail
/// latency never starves the async workers. `None` when not armed (see
/// [`drm_reconcile_schedule_from_env`] for the arming rules).
///
/// This wrapper is the only untested shim: the tick body, the arming decision, the bounding, the
/// panic isolation, and the idempotency are all CI-driven directly with a mock confirmer.
pub fn spawn_drm_reconcile_scheduler(
    rail: Option<&PayRail>,
    audit_log: Arc<AuditLog>,
) -> Option<tokio::task::JoinHandle<()>> {
    let rail = rail?;
    let (interval, batch) = drm_reconcile_schedule_from_env(rail.drm_confirmer.is_some())?;
    let confirmer = rail.drm_confirmer.clone()?; // present — the arming check requires it
    let meter = rail.meter.clone();
    let ledger = rail.ledger.clone();
    tracing::info!(
        "DRM confirmation scheduler ARMED: every {}s, up to {batch} pending buys per tick \
         (promote at the confirmation floor / refund a revert / hold the rest)",
        interval.as_secs()
    );
    Some(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // A stalled tick (slow RPC) must not cause a burst of catch-up ticks against the chain.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The rotating scan cursor (council S37 F1): each tick starts after the previous tick's
        // last scanned seq, so a stuck-Unconfirmed prefix can never starve the entries behind it.
        let mut cursor: Option<u64> = None;
        // At most ONE tick in flight (council S37 guardian F2): the chain-provider read has no
        // deadline today, so a hung RPC would otherwise either wedge the schedule (awaiting it)
        // or stack a new blocked thread per interval (not awaiting it). Instead: if the previous
        // tick is still running, LOG LOUDLY and skip — the schedule stays alive and observable,
        // blocked threads stay bounded at one, and every entry stays safely Pending.
        let mut in_flight: Option<
            tokio::task::JoinHandle<crate::drm_marketplace::DrmReconcileSummary>,
        > = None;
        loop {
            ticker.tick().await;
            if let Some(handle) = in_flight.take() {
                if !handle.is_finished() {
                    tracing::error!(
                        "a previous DRM confirmation tick is STILL RUNNING (hung chain RPC?) — \
                         skipping this interval; pending buys stay held until it returns"
                    );
                    in_flight = Some(handle);
                    continue;
                }
                match handle.await {
                    Ok(summary) => {
                        if summary.next_cursor.is_some() {
                            cursor = summary.next_cursor;
                        }
                        if summary.scanned() > 0 || summary.skipped > 0 {
                            tracing::info!(
                                promoted = summary.promoted,
                                refunded = summary.refunded,
                                left_pending = summary.left_pending,
                                skipped = summary.skipped,
                                "DRM confirmation tick"
                            );
                        }
                    }
                    Err(e) => {
                        // The tick task itself died (per-entry panics are isolated inside).
                        // Hold everything (entries stay Pending) and keep the schedule alive.
                        tracing::error!("DRM confirmation tick aborted: {e} — retrying next tick");
                    }
                }
            }
            let meter = meter.clone();
            let ledger = ledger.clone();
            let audit_log = audit_log.clone();
            let confirmer = confirmer.clone();
            let start_after = cursor;
            in_flight = Some(tokio::task::spawn_blocking(move || {
                crate::drm_marketplace::drm_reconcile_tick(
                    &ledger,
                    &meter,
                    &audit_log,
                    confirmer.as_ref(),
                    batch,
                    start_after,
                )
            }));
        }
    }))
}

pub struct HostHelperProcess {
    pub name: &'static str,
    pub child: Child,
}

impl Drop for HostHelperProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        tracing::info!("{} host helper stopped", self.name);
    }
}

/// Start the HTTP API server with full session and capability support
///
/// This is the Phase 5+ server configuration that includes:
/// - Session token authentication
/// - Capability request/grant/deny flow
/// - Shell-only endpoints for permission management
/// - Namespace API for content-addressed storage
/// - File-backed localhost API (localhost://<root>/...)
pub async fn start_server_with_sessions(config: ServerConfig) -> anyhow::Result<()> {
    let ServerConfig {
        runtime,
        session_registry,
        capability_manager,
        standing_service,
        pending_store,
        namespace_store,
        provider_registry,
        audit_log,
        identity_state,
        docs_dir,
        addr,
        capsule_dir,
        data_dir,
        bootstrap_state,
        tls_config,
        supervisor,
        ready_tx,
        attach_secret,
        host_helpers,
        pay_rail,
    } = config;
    let _host_helpers = host_helpers;
    // CORS: allow localhost origins for browser-based capsule UIs and
    // local development. Parses the Origin URL and compares the host
    // to prevent bypass via domains like localhost.evil.com.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            is_allowed_local_origin(origin)
        }))
        .allow_methods(Any)
        .allow_headers(Any);

    // Shared state
    let api_state = ApiState {
        session_registry: session_registry.clone(),
    };

    let shadow_mode = std::env::var("ELASTOS_SHADOW_MODE")
        .unwrap_or_default()
        .to_ascii_lowercase();

    let policy_evaluator = match shadow_mode.as_str() {
        "rules" => {
            tracing::info!("Shadow verification enabled (RulesVerifier)");
            Arc::new(PolicyEvaluator::with_shadow(
                Box::new(ShellPassthroughVerifier),
                Box::new(RulesVerifier::with_defaults()),
                capability_manager.audit_log().clone(),
            ))
        }
        "1" | "true" | "yes" | "on" => {
            tracing::info!("Shadow verification enabled (AutoGrantVerifier)");
            Arc::new(PolicyEvaluator::with_shadow(
                Box::new(ShellPassthroughVerifier),
                Box::new(AutoGrantVerifier),
                capability_manager.audit_log().clone(),
            ))
        }
        _ => Arc::new(PolicyEvaluator::new(
            Box::new(ShellPassthroughVerifier),
            capability_manager.audit_log().clone(),
        )),
    };

    let capability_state = CapabilityState {
        pending_store: pending_store.clone(),
        capability_manager: capability_manager.clone(),
        policy_evaluator,
        // The shared runtime mandate registry when provided (so the Home gateway's Mandates app
        // sees the same grants); otherwise this server's own. All shell-only standing-grant verbs
        // hit this one fail-closed registry. The fallback is LOUD (guardian F6): a memory-only
        // registry loses mandates AND the intent replay guard on restart (G-M5 durability) — every
        // production constructor passes the shared persistent service, so hitting this warns of a
        // mis-wired caller, not a normal path.
        standing_service: standing_service.unwrap_or_else(|| {
            tracing::warn!(
                "no shared mandate registry provided — falling back to a MEMORY-ONLY registry: \
                 mandates and the intent replay guard will NOT survive restart"
            );
            Arc::new(capability_manager.standing_grant_service())
        }),
        // `runtime.notify` delivers into the operator's Inbox store under data_dir; without one
        // (bare test/embedded configs) the method is honestly unwired => Undelivered.
        //
        // `runtime.pay` wiring (Sprint 31): the rail is SELECTED AND BUILT by [`build_pay_rail`]
        // in the infrastructure layer (one PayRail per process, keyed off the infra's real
        // data_dir — NOT this config's `data_dir` field), and arrives here via
        // `ServerConfig.pay_rail`. `None` ⇒ pay honestly unwired and the provisioning/reconcile
        // surfaces answer 503. See build_pay_rail's doc for the fail-closed selection rules
        // (https-validated real endpoint > env-gated mock > unwired; durable stores required).
        intent_executor: {
            let base = crate::intent_executor::MethodRegistryExecutor::production(
                capability_manager.audit_log().clone(),
                data_dir.clone(),
            );
            match &pay_rail {
                Some(rail) => Arc::new(base.with_payments(
                    rail.meter.clone(),
                    rail.provider.clone(),
                    rail.ledger.clone(),
                )),
                None => Arc::new(base),
            }
        },
        spend_meter: pay_rail.as_ref().map(|r| r.meter.clone()),
        payment_ledger: pay_rail.as_ref().map(|r| r.ledger.clone()),
    };
    let capsule_audit_log = audit_log
        .clone()
        .unwrap_or_else(|| Arc::new(AuditLog::new()));

    // Rate limiters: 100 req/s general, 5 req/s for identity endpoints
    let general_rate_limiter = Arc::new(RateLimiter::new(100.0));
    let identity_rate_limiter = Arc::new(RateLimiter::new(5.0));

    let general_rate_state = RateLimitState {
        session_registry: session_registry.clone(),
        rate_limiter: general_rate_limiter,
    };

    let identity_rate_state = RateLimitState {
        session_registry: session_registry.clone(),
        rate_limiter: identity_rate_limiter,
    };

    // Public routes (no auth required)
    let public_routes = Router::new().route("/api/health", get(routes::health));

    // Attach endpoint — exchanges local secret for a session token.
    let attach_routes = if let Some(secret) = attach_secret {
        let attach_state = handlers::attach::AttachState {
            session_registry: session_registry.clone(),
            secret,
        };
        Router::new()
            .route("/api/auth/attach", post(handlers::attach::attach))
            .with_state(attach_state)
    } else {
        Router::new()
    };

    // Authenticated routes (require valid session token, rate-limited)
    let auth_routes = Router::new()
        .route("/api/session", get(handlers::session_info))
        .route(
            "/api/capability/request",
            post(handlers::request_capability),
        )
        .route("/api/capability/request/:id", get(handlers::request_status))
        .route("/api/capability/list", get(handlers::list_capabilities))
        .route(
            "/api/capability/validate-and-consume",
            post(handlers::validate_and_consume),
        )
        .layer(axum_middleware::from_fn_with_state(
            general_rate_state.clone(),
            rate_limit_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(capability_state.clone());

    // Shell-only routes (require shell session)
    let shell_routes = Router::new()
        .route("/api/capability/pending", get(handlers::list_pending))
        .route("/api/capability/grant", post(handlers::grant_request))
        .route("/api/capability/deny", post(handlers::deny_request))
        // Standing grants (unsupervised-agent authority): issue mints a real token + stores the
        // standing envelope; revoke is the kill switch. Shell-only, like grant/deny.
        .route(
            "/api/standing-grants/issue",
            post(handlers::issue_standing_grant),
        )
        // The operator's mandate list (revoked included, flagged) + the runtime's signer pin.
        .route("/api/standing-grants", get(handlers::list_standing_grants))
        // The ACT leg: run one authenticated agent intent under its standing mandate,
        // fail-closed (declaration recorded before the act; token-keyed use recorded after).
        .route(
            "/api/standing-grants/dispatch",
            post(handlers::dispatch_standing_intent),
        )
        .route(
            "/api/standing-grants/revoke",
            post(handlers::revoke_standing_grant),
        )
        // Read-only dry-run: does a SIGNED intent fall within its standing grant? Records nothing.
        .route(
            "/api/standing-grants/preview",
            post(handlers::preview_standing_grant),
        )
        // Per-mandate receipt: the portable, set-bound audit bundle for ONE capability token —
        // read-only over the durable chain, verified off-box with `elastos verify-receipt`.
        .route(
            "/api/mandate/:token_id/receipt",
            get(handlers::mandate_receipt),
        )
        // Spend budgets (Sprint 28): the operator's money-cap provisioning surface for runtime.pay.
        // Shell-only like issue/revoke — a budget is real operator authority. Fail-closed: refused
        // without a wired rail or on a non-durable meter; attested on the audit chain when applied.
        .route("/api/spend-budgets", post(handlers::set_spend_budget))
        .route(
            "/api/spend-budgets/:capsule",
            get(handlers::get_spend_budget),
        )
        // Payment reconciliation (Sprint 30): the operator resolves INDETERMINATE payments against
        // the rail's verdict — the only path that releases an indeterminate reservation. Shell-only
        // like the budget surface; each resolution is single-shot and attested on the chain.
        .route(
            "/api/payments/pending",
            get(handlers::list_pending_payments),
        )
        .route("/api/payments/reconcile", post(handlers::reconcile_payment))
        // Revoke endpoints
        .route("/api/capability/:id", delete(handlers::revoke_capability))
        .route(
            "/api/capability/revoke-all",
            post(handlers::revoke_all_capabilities),
        )
        // Audit log endpoints
        .route("/api/audit", get(handlers::get_audit_log))
        .route("/api/audit/types", get(handlers::get_audit_event_types))
        .layer(axum_middleware::from_fn(consent_broker_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(capability_state.clone());

    // Agent-facing dispatch (Sprint 26) — the ACT leg reachable by the AGENT, not only the operator
    // shell. Deliberately NOT behind `consent_broker_only_middleware`: the agent authenticates AS the
    // mandate holder (the signed intent + the mandate's agent-key binding, G-M4), so no shell role is
    // needed. `auth_middleware` still requires a valid session (transport auth / anti-anonymous DoS),
    // and the general rate limit bounds per-session request volume; the handler then requires a BOUND
    // mandate + a matching signer before any act (charge-on-authorized). This is the "a mandate, not
    // your keys" surface.
    let agent_routes = Router::new()
        .route(
            "/api/agent/dispatch",
            post(handlers::dispatch_agent_intent),
        )
        .layer(axum_middleware::from_fn_with_state(
            general_rate_state.clone(),
            rate_limit_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(capability_state.clone());

    // Orchestrator routes (shell-only — runtime coordination for attach flow)
    let orchestrator_state = handlers::orchestrator::OrchestratorState {
        session_registry: session_registry.clone(),
    };
    let orchestrator_routes = Router::new()
        .route(
            "/api/orchestrator/session",
            post(handlers::orchestrator::create_session),
        )
        .layer(axum_middleware::from_fn(consent_broker_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .with_state(orchestrator_state);

    // Supervisor routes (shell-only — capsule lifecycle for VM-based supervisor path)
    let supervisor_routes = if let Some(sup) = supervisor {
        let sup_state = handlers::supervisor_api::SupervisorState {
            supervisor: sup,
            data_dir: data_dir.clone(),
        };
        Router::new()
            .route(
                "/api/supervisor/ensure-external",
                post(handlers::supervisor_api::ensure_external),
            )
            .route(
                "/api/supervisor/ensure-capsule",
                post(handlers::supervisor_api::ensure_capsule),
            )
            .route(
                "/api/supervisor/launch-capsule",
                post(handlers::supervisor_api::launch_capsule),
            )
            .route(
                "/api/supervisor/stop-capsule",
                post(handlers::supervisor_api::stop_capsule),
            )
            .route(
                "/api/supervisor/wait-capsule",
                post(handlers::supervisor_api::wait_capsule),
            )
            .route(
                "/api/supervisor/resolve-plan",
                post(handlers::supervisor_api::resolve_plan),
            )
            .route(
                "/api/supervisor/start-gateway",
                post(handlers::supervisor_api::start_gateway),
            )
            .layer(axum_middleware::from_fn(consent_broker_only_middleware))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(sup_state)
    } else {
        Router::new()
    };

    // Namespace routes (require valid session, optional - only if namespace_store is provided)
    let namespace_routes = if let Some(ns_store) = namespace_store {
        let namespace_state = NamespaceState {
            namespace_store: ns_store,
            capability_manager: Some(capability_manager.clone()),
        };

        Router::new()
            .route("/api/namespace/list", get(handlers::list_path))
            .route("/api/namespace/resolve", get(handlers::resolve_path))
            .route("/api/namespace/read", get(handlers::read_content))
            .route("/api/namespace/write", post(handlers::write_content))
            .route("/api/namespace/delete", delete(handlers::delete_path))
            .route("/api/namespace/status", get(handlers::namespace_status))
            .route("/api/namespace/cache", get(handlers::cache_status))
            .route("/api/namespace/prefetch", post(handlers::prefetch_content))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(namespace_state)
    } else {
        Router::new()
    };

    // File-backed localhost routes (require valid session, optional - only if provider_registry is provided)
    // Public contract: rooted `localhost://...` paths.
    let storage_routes = if let Some(registry) = provider_registry {
        let storage_state = handlers::storage::ProviderStorageState {
            registry: registry.clone(),
            audit_log: audit_log.clone(),
            capability_manager: Some(capability_manager.clone()),
            storage_quota_mb: 0, // 0 = unlimited (configurable via RuntimeConfig)
        };

        // Generic provider proxy: POST /api/provider/:scheme/:op
        let proxy_state = handlers::provider::ProviderProxyState {
            registry,
            capability_manager: Some(capability_manager.clone()),
        };

        let storage_router = Router::new()
            .route("/api/localhost", get(handlers::storage_get_root))
            .route("/api/localhost/", get(handlers::storage_get_root))
            .route("/api/localhost/*path", get(handlers::storage_get))
            .route("/api/localhost/*path", put(handlers::storage_write))
            .route("/api/localhost/*path", delete(handlers::storage_delete))
            .route("/api/localhost/*path", head(handlers::storage_stat))
            .route("/api/localhost/*path", post(handlers::storage_post))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(storage_state);

        // Generic provider proxy route
        let proxy_router = Router::new()
            .route(
                "/api/provider/:scheme/:op",
                post(handlers::provider::provider_proxy),
            )
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state);

        storage_router.merge(proxy_router)
    } else {
        Router::new()
    };

    // Identity routes (require valid session, stricter rate limit: 5 req/s)
    let identity_routes = if let Some(id_state) = identity_state {
        Router::new()
            .route(
                "/api/identity/status",
                get(handlers::identity::identity_status),
            )
            .route(
                "/api/identity/register/begin",
                post(handlers::identity::register_begin),
            )
            .route(
                "/api/identity/register/complete",
                post(handlers::identity::register_complete),
            )
            .route(
                "/api/identity/authenticate/begin",
                post(handlers::identity::authenticate_begin),
            )
            .route(
                "/api/identity/authenticate/complete",
                post(handlers::identity::authenticate_complete),
            )
            .layer(axum_middleware::from_fn_with_state(
                identity_rate_state,
                rate_limit_middleware,
            ))
            .layer(axum_middleware::from_fn_with_state(
                api_state.clone(),
                auth_middleware,
            ))
            .with_state(id_state)
    } else {
        Router::new()
    };

    // Documentation routes (no auth, read-only)
    let docs_routes = if let Some(dir) = docs_dir {
        let docs_state = DocsState {
            docs_dir: Arc::new(dir),
        };
        Router::new()
            .route("/api/docs", get(handlers::docs::list_docs))
            .route("/api/docs/{name}", get(handlers::docs::get_doc))
            .with_state(docs_state)
    } else {
        Router::new()
    };

    // Bootstrap route (no auth — localhost only, returns app token + capsule info)
    let bootstrap_routes = if let Some(bs) = bootstrap_state {
        Router::new().route(
            "/api/capsule/bootstrap",
            get({
                let bs = bs.clone();
                move || async move {
                    axum::Json(serde_json::json!({
                        "token": bs.token,
                        "name": bs.manifest.name,
                        "rom": bs.manifest.entrypoint,
                        "storage": bs.manifest.permissions.storage,
                    }))
                }
            }),
        )
    } else {
        Router::new()
    };

    // Capsule management routes (require shell session — launching/stopping is an orchestrator operation)
    let capsule_mgmt_routes = Router::new()
        .route("/api/capsules", get(routes::list_capsules))
        .route("/api/capsules", post(routes::launch_capsule))
        .route("/api/capsules/:id", delete(routes::stop_capsule))
        .layer(axum_middleware::from_fn(consent_broker_only_middleware))
        .layer(axum_middleware::from_fn_with_state(
            api_state.clone(),
            auth_middleware,
        ))
        .layer(Extension(data_dir.clone()))
        .layer(Extension(capsule_audit_log))
        .layer(Extension(runtime));

    // Combine all routes
    let mut app = Router::new()
        .merge(public_routes)
        .merge(attach_routes)
        .merge(auth_routes)
        .merge(shell_routes)
        .merge(agent_routes)
        .merge(orchestrator_routes)
        .merge(supervisor_routes)
        .merge(namespace_routes)
        .merge(storage_routes)
        .merge(identity_routes)
        .merge(docs_routes)
        .merge(bootstrap_routes)
        .merge(capsule_mgmt_routes)
        .layer(cors.clone());

    // Add test endpoints in debug builds
    #[cfg(debug_assertions)]
    {
        use crate::api::handlers::test_helpers::{create_test_session, TestState};

        let test_state = TestState {
            session_registry: session_registry.clone(),
        };

        let test_routes = Router::new()
            .route("/api/test/create-session", post(create_test_session))
            .with_state(test_state);

        app = app.merge(test_routes);
        tracing::info!("Test endpoints enabled (debug build)");
    }

    // Add data capsule file serving at /capsule-data/ if data_dir is provided
    if let Some(ref dir) = data_dir {
        tracing::info!("Serving capsule data from: {}", dir.display());
        let data_serve = ServeDir::new(dir);
        app = app.nest_service("/capsule-data", data_serve);
    }

    // Add static file serving for web capsules if directory is provided
    let has_capsule = capsule_dir.is_some();
    if let Some(dir) = capsule_dir {
        tracing::info!("Serving web capsule from: {}", dir.display());
        let serve_dir = ServeDir::new(&dir).append_index_html_on_directories(true);
        app = app.fallback_service(serve_dir);
    }

    // Apply COOP/COEP headers to ALL responses when serving a web capsule.
    // Must be after nest_service/fallback_service so it wraps everything.
    if has_capsule {
        app = app.layer(axum_middleware::from_fn(cross_origin_isolation));
    }

    // Start server with or without TLS
    if let Some(tls_config) = tls_config {
        let socket_addr: std::net::SocketAddr = addr.parse()?;
        tracing::info!("API server listening on https://{} (TLS + sessions)", addr);
        if has_capsule {
            tracing::info!("Web capsule available at https://{}", addr);
        }
        // Signal readiness before blocking on serve (TLS bind is implicit)
        if let Some(tx) = ready_tx {
            let _ = tx.send(());
        }
        axum_server::bind_rustls(socket_addr, tls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("API server listening on http://{} (sessions enabled)", addr);
        if has_capsule {
            tracing::info!("Web capsule available at http://{}", addr);
        }
        // Signal readiness after successful bind, before blocking on serve
        if let Some(tx) = ready_tx {
            let _ = tx.send(());
        }
        axum::serve(listener, app).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_pay_rail;
    use super::drm_reconcile_schedule_from_env;
    use super::is_allowed_local_origin;
    use axum::http::HeaderValue;

    /// Both `build_pay_rail` tests mutate PROCESS-GLOBAL payment env vars (they overlap on
    /// `ELASTOS_ALLOW_MOCK_PAYMENTS`), so they must run one-at-a-time — Rust runs tests in
    /// parallel threads by default. This lock serializes them (`.lock()` at the top of each).
    static PAY_RAIL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn allows_local_loopback_origins() {
        assert!(is_allowed_local_origin(&HeaderValue::from_static(
            "http://localhost:3000"
        )));
        assert!(is_allowed_local_origin(&HeaderValue::from_static(
            "http://127.0.0.1:3000"
        )));
        assert!(is_allowed_local_origin(&HeaderValue::from_static(
            "http://[::1]:3000"
        )));
    }

    #[test]
    fn rejects_non_local_origins() {
        assert!(!is_allowed_local_origin(&HeaderValue::from_static(
            "http://localhost.evil.com"
        )));
        assert!(!is_allowed_local_origin(&HeaderValue::from_static(
            "https://example.com"
        )));
    }

    /// Save the named env vars, REMOVE them for the test, and restore the originals on drop —
    /// the one shared guard for env-mutating pay-rail tests (hold the right lock(s) first).
    struct EnvGuard(Vec<(&'static str, Option<String>)>);
    impl EnvGuard {
        fn capture(keys: &[&'static str]) -> Self {
            let saved = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            for k in keys {
                std::env::remove_var(k);
            }
            Self(saved)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    /// Council S31 G-F10: the boot rail selection was extractable and untested. One SEQUENTIAL
    /// test (env vars are process-global) covering: unset ⇒ unwired; mock ⇒ durable stores;
    /// second build in the same data_dir ⇒ flock-refused ⇒ unwired; plaintext non-loopback real
    /// endpoint ⇒ refused; https real endpoint ⇒ wired; real wins over mock.
    #[test]
    fn build_pay_rail_selects_fail_closed() {
        let _serial = PAY_RAIL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // ELASTOS_PAYMENT_RAIL is guarded too: an inherited `=drm` from the shell would silently
        // flip every assertion below into the DRM branch.
        let _guard = EnvGuard::capture(&[
            "ELASTOS_PAYMENT_RAIL",
            "ELASTOS_PAYMENT_ENDPOINT",
            "ELASTOS_PAYMENT_TOKEN",
            "ELASTOS_ALLOW_MOCK_PAYMENTS",
        ]);

        // No envs ⇒ honestly unwired.
        let dir = tempfile::tempdir().unwrap();
        assert!(build_pay_rail(Some(dir.path())).is_none());

        // Mock env + data_dir ⇒ wired with DURABLE stores.
        std::env::set_var("ELASTOS_ALLOW_MOCK_PAYMENTS", "1");
        let rail = build_pay_rail(Some(dir.path())).expect("mock rail wires");
        assert!(rail.meter.is_durable());

        // A second build in the SAME data_dir while the first is alive ⇒ the stores' flocks
        // refuse ⇒ unwired (fail-closed, never a second opener clobbering spent).
        assert!(
            build_pay_rail(Some(dir.path())).is_none(),
            "double-build refuses via the single-opener flocks"
        );
        drop(rail);

        // Plaintext http to a non-loopback host ⇒ refused at boot.
        std::env::set_var("ELASTOS_PAYMENT_ENDPOINT", "http://pay.example.com/orders");
        assert!(build_pay_rail(Some(dir.path())).is_none());
        // Malformed ⇒ refused at boot.
        std::env::set_var("ELASTOS_PAYMENT_ENDPOINT", "not a url");
        assert!(build_pay_rail(Some(dir.path())).is_none());

        // https ⇒ the REAL rail wires (and wins over the still-set mock env).
        std::env::set_var("ELASTOS_PAYMENT_ENDPOINT", "https://pay.example.com/orders");
        let rail = build_pay_rail(Some(dir.path())).expect("https real rail wires");
        assert!(rail.meter.is_durable());
        drop(rail);

        // Real endpoint set but NO durable stores possible (no data_dir) ⇒ refused.
        assert!(
            build_pay_rail(None).is_none(),
            "real money on non-durable stores is refused"
        );
    }

    /// Council S34 guardian F4: the DRM rail obeys the S29 mock-money discipline. Under the secure
    /// `Chain` rights mode it wires on the durable stores; under a non-Chain (synthetic-tx) mode it
    /// wires ONLY with the explicit `ELASTOS_ALLOW_MOCK_PAYMENTS` opt-in, else stays UNWIRED.
    #[test]
    fn drm_rail_obeys_the_mock_money_discipline() {
        // LOCK ORDER: the crate-wide ddrm env lock FIRST (this test mutates ELASTOS_DDRM_RIGHTS,
        // a process-global the rights/mint/owned-ledger authority tests also guard with it —
        // PAY_RAIL_ENV_LOCK alone only serializes this module against itself), then the pay-rail
        // lock. Every test that takes both must take them in this order.
        let _ddrm = crate::api::ddrm_env_lock();
        let _serial = PAY_RAIL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _guard = EnvGuard::capture(&[
            "ELASTOS_PAYMENT_RAIL",
            "ELASTOS_PAYMENT_ENDPOINT",
            "ELASTOS_ALLOW_MOCK_PAYMENTS",
            "ELASTOS_DDRM_RIGHTS",
            "ELASTOS_DRM_SPEND_UNIT",
            "ELASTOS_DRM_PAY_TOKEN",
        ]);

        std::env::set_var("ELASTOS_PAYMENT_RAIL", "drm");

        // Secure Chain mode WITHOUT the declared unit mapping ⇒ refuses to wire (Sprint 36).
        std::env::set_var("ELASTOS_DDRM_RIGHTS", "chain");
        std::env::remove_var("ELASTOS_DRM_SPEND_UNIT");
        let dir0 = tempfile::tempdir().unwrap();
        assert!(
            build_pay_rail(Some(dir0.path())).is_none(),
            "the live DRM rail refuses to wire without ELASTOS_DRM_SPEND_UNIT (fail-closed)"
        );
        // Unit declared but STILL no pay-token ⇒ refuses (Sprint 36 F3 — the unit maps one token).
        std::env::set_var("ELASTOS_DRM_SPEND_UNIT", "1000000");
        let dir1 = tempfile::tempdir().unwrap();
        assert!(
            build_pay_rail(Some(dir1.path())).is_none(),
            "the live DRM rail refuses to wire without ELASTOS_DRM_PAY_TOKEN (fail-closed)"
        );
        // Both the unit AND the pay-token declared ⇒ the DRM rail wires on the durable stores.
        std::env::set_var("ELASTOS_DRM_PAY_TOKEN", "0xUSDC");
        let dir = tempfile::tempdir().unwrap();
        let rail = build_pay_rail(Some(dir.path())).expect("DRM rail wires under Chain mode");
        assert!(rail.meter.is_durable());
        drop(rail);

        // Under a non-Chain (synthetic-settlement) rights mode, wiring requires the explicit
        // mock opt-in — otherwise UNWIRED, fail-closed. Reachable only in a dev-modes build (a
        // release build cannot leave Chain), so the negative case is gated on the feature.
        #[cfg(feature = "dev-modes")]
        {
            std::env::set_var("ELASTOS_DDRM_RIGHTS", "dev");
            std::env::remove_var("ELASTOS_ALLOW_MOCK_PAYMENTS");
            let dir2 = tempfile::tempdir().unwrap();
            assert!(
                build_pay_rail(Some(dir2.path())).is_none(),
                "DRM rail under a synthetic mode without the mock opt-in stays UNWIRED"
            );
            std::env::set_var("ELASTOS_ALLOW_MOCK_PAYMENTS", "1");
            let dir3 = tempfile::tempdir().unwrap();
            let rail = build_pay_rail(Some(dir3.path()))
                .expect("with the explicit mock opt-in, the dev DRM rail wires");
            assert!(rail.meter.is_durable());
        }
    }
    /// Sprint 37 ratchet: the scheduler's fail-closed arming rules. OFF by default (no interval
    /// env ⇒ None); a malformed interval or batch REFUSES to arm (never guesses); an interval on
    /// a non-DRM rail stays off; armed only on interval + DRM confirmer, with batch defaulting
    /// to 64.
    #[test]
    fn the_drm_scheduler_arms_only_with_an_interval_and_a_drm_rail() {
        let _serial = PAY_RAIL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _guard = EnvGuard::capture(&[
            "ELASTOS_DRM_RECONCILE_INTERVAL_SECS",
            "ELASTOS_DRM_RECONCILE_BATCH",
        ]);

        // OFF BY DEFAULT: no interval declared ⇒ never armed, DRM rail or not.
        assert!(drm_reconcile_schedule_from_env(true).is_none());
        assert!(drm_reconcile_schedule_from_env(false).is_none());

        // Malformed interval ⇒ fail-closed OFF (no guessed cadence).
        std::env::set_var("ELASTOS_DRM_RECONCILE_INTERVAL_SECS", "soon");
        assert!(drm_reconcile_schedule_from_env(true).is_none());
        std::env::set_var("ELASTOS_DRM_RECONCILE_INTERVAL_SECS", "0");
        assert!(drm_reconcile_schedule_from_env(true).is_none());

        // A valid interval on a NON-DRM rail ⇒ off (nothing on-chain to poll).
        std::env::set_var("ELASTOS_DRM_RECONCILE_INTERVAL_SECS", "30");
        assert!(drm_reconcile_schedule_from_env(false).is_none());

        // Valid interval + DRM rail ⇒ armed, batch defaults to 64.
        let (interval, batch) =
            drm_reconcile_schedule_from_env(true).expect("interval + DRM rail arms");
        assert_eq!(interval, std::time::Duration::from_secs(30));
        assert_eq!(batch, 64, "the per-tick bound defaults");

        // Explicit batch is honored; a malformed batch refuses to arm (no guessed bound).
        std::env::set_var("ELASTOS_DRM_RECONCILE_BATCH", "5");
        assert_eq!(drm_reconcile_schedule_from_env(true).unwrap().1, 5);
        std::env::set_var("ELASTOS_DRM_RECONCILE_BATCH", "many");
        assert!(drm_reconcile_schedule_from_env(true).is_none());
        std::env::set_var("ELASTOS_DRM_RECONCILE_BATCH", "0");
        assert!(drm_reconcile_schedule_from_env(true).is_none());
    }
}
