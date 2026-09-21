//! HTTP API module
//!
//! This module provides the HTTP API for the ElastOS runtime:
//! - Session authentication via bearer tokens
//! - Capability request/grant/deny flow
//! - Health and status endpoints

pub mod auth_gateway;
pub mod browser_capsules;
pub mod browser_engine_protocol;
pub mod browser_sessions;
pub(crate) mod capsule_inventory;
// Runtime startup composition; this is not a capsule operation or HTTP route.
#[cfg(unix)]
pub use capsule_inventory::preparation::{
    append_admitted_model_startup_offers, settle_pending_model_startup,
};
pub mod gateway;
pub(crate) mod gateway_local_control;
pub mod handlers;
pub mod middleware;
mod model_provider_config;
#[cfg(test)]
pub(crate) use model_provider_config::seed_model_provider_operator_offers_for_test;
pub(crate) use model_provider_config::{
    ai_provider_status, any_hosted_model_shared, hosted_model_offer_hint, hosted_model_share_cards,
    load_model_provider_operator_offers, named_jev_hosted_offer, offer_is_shareable,
    operator_has_hosted_offer, remove_hosted_offer, save_hosted_offer, set_hosted_offer_share,
    HostedAiProvider, HostedModelOfferHint,
};
#[cfg(not(test))]
pub(crate) use model_provider_config::{load_hosted_validate_fixtures, parse_loopback_http_url};
pub use model_provider_config::{model_provider_bridge_config, model_provider_config};
pub mod routes;
pub mod server;
pub mod viewer_gateway;
