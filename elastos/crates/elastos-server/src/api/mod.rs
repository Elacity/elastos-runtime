//! HTTP API module
//!
//! This module provides the HTTP API for the ElastOS runtime:
//! - Session authentication via bearer tokens
//! - Capability request/grant/deny flow
//! - Health and status endpoints

pub mod auth_gateway;
pub mod browser_capsules;
pub mod browser_sessions;
pub mod buy_authority;
pub(crate) mod capsule_inventory;
pub mod chain_tx;
pub mod creator;
pub mod gateway;
pub mod handlers;
pub mod media_authority;
pub mod middleware;
pub mod mint_authority;
pub mod object_authority;
pub mod owned_ledger;
pub mod rights_authority;
pub mod routes;
pub mod server;
pub mod viewer_gateway;
pub mod viewer_media;
pub mod viewer_object;
pub mod viewer_open;
pub mod wallet_signer;
