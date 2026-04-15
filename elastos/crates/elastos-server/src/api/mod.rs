//! HTTP API module
//!
//! This module provides the HTTP API for the ElastOS runtime:
//! - Session authentication via bearer tokens
//! - Capability request/grant/deny flow
//! - Health and status endpoints

pub mod browser_capsules;
pub mod gateway;
pub mod handlers;
pub mod middleware;
pub mod routes;
pub mod server;
