//! ElastOS Server
//!
//! HTTP API, CLI orchestration, and capsule loading for ElastOS.
//! This crate provides the transport layer (HTTP) and binary entry point.
//! The security-critical runtime logic lives in `elastos-runtime`.

pub mod api;
pub mod auth;
pub mod binaries;
pub mod browser_app_hosts;
pub mod carrier;
pub mod carrier_bridge;
pub mod carrier_service;
pub mod content;
pub mod crypto;
pub mod documents;
pub mod fetcher;
pub mod gateway_cmd;
pub mod host_lock;
pub mod init;
pub mod ipfs;
pub mod library;
pub mod local_http;
pub mod notifications;
pub mod operator_control;
pub mod ownership;
pub mod provider_resource;
pub mod room_service;
pub mod runtime;
pub mod runtime_control;
pub mod setup;
pub mod shares;
pub mod shell_cmd;
pub mod sources;
pub mod supervisor;
pub mod update;
pub mod vm_provider;
