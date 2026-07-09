//! HTTP API module
//!
//! This module provides the HTTP API for the ElastOS runtime:
//! - Session authentication via bearer tokens
//! - Capability request/grant/deny flow
//! - Health and status endpoints

pub mod access_grant;
pub mod auth_gateway;
pub mod browser_capsules;
pub mod browser_sessions;
pub mod buy_authority;
pub(crate) mod capsule_inventory;
pub(crate) mod capsule_watchdog;
pub mod chain_tx;
pub mod content_index;
pub mod creator;
pub mod erc20_checkout;
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
pub mod trade_authority;
pub mod viewer_gateway;
pub mod viewer_media;
pub mod viewer_object;
pub mod viewer_open;
pub mod wallet_signer;

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
