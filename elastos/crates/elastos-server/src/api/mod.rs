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
pub use capsule_inventory::preparation::append_admitted_model_startup_offers;
pub mod gateway;
pub(crate) mod gateway_local_control;
pub mod handlers;
pub mod middleware;
mod model_provider_config;
pub use model_provider_config::{model_provider_bridge_config, model_provider_config};
pub mod routes;
pub mod server;
pub mod viewer_gateway;
