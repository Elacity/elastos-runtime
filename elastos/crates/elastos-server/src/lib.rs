//! ElastOS Server
//!
//! HTTP API, CLI orchestration, and capsule loading for ElastOS.
//! This crate provides the transport layer (HTTP) and binary entry point.
//! The security-critical runtime logic lives in `elastos-runtime`.

pub mod api;
pub mod binaries;
pub mod carrier;
pub mod carrier_bridge;
pub mod carrier_service;
pub mod crypto;
pub mod fetcher;
pub mod gateway_cmd;
pub mod init;
pub mod ipfs;
pub mod local_http;
pub mod ownership;
pub mod runtime;
pub mod setup;
pub mod shares;
pub mod sources;
pub mod supervisor;
pub mod update;
pub mod vm_provider;
